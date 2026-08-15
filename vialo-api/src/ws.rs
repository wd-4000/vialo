//! WebSocket server.
//! Handles event subscriptions

use axum::{
    Extension, Router,
    body::Bytes,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    middleware,
    response::IntoResponse,
    routing::any,
};
use axum_extra::TypedHeader;
use serde::Deserialize;
use std::net::SocketAddr;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing::debug;

//allows to extract the IP of connecting user
use axum::extract::connect_info::ConnectInfo;

//allows to split the websocket stream into separate TX and RX branches
use crate::AppState;
use crate::http::util::{User, middleware::auth_middleware};
use futures::{sink::SinkExt, stream::StreamExt};

/// Subscription first, so a frame carrying both channel/id and auth_key can't
/// be mistaken for the auth-only variant
#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum WsFrame {
    Subscription { channel: String, id: u16 },
    Auth { auth_key: String },
}

pub fn main(app_state: Arc<AppState>) -> Router {
    Router::new()
        .route("/ws", any(ws_handler))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            auth_middleware,
        ))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::default().include_headers(true)),
        )
        .with_state(app_state)
}

/// The handler for the HTTP request (this gets called when the HTTP request lands at the start
/// of websocket negotiation). After this completes, the actual switching from HTTP to
/// websocket protocol will occur.
/// This is the last point where we can extract TCP/IP metadata such as IP address of the client
/// as well as things from HTTP headers such as user-agent of the browser etc.
async fn ws_handler(
    ws: WebSocketUpgrade,
    user_agent: Option<TypedHeader<headers::UserAgent>>,
    State(state): State<Arc<AppState>>,
    addr: Option<Extension<ConnectInfo<SocketAddr>>>,
    Extension(user): Extension<Option<User>>,
) -> impl IntoResponse {
    let addr = addr
        .map(|Extension(c)| c.0)
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 0)));
    let user_agent = if let Some(TypedHeader(user_agent)) = user_agent {
        user_agent.to_string()
    } else {
        String::from("Unknown browser")
    };
    debug!("`{user_agent}` at {addr} connected.");
    ws.on_upgrade(move |socket| handle_socket(socket, addr, state, user))
}

/// Actual websocket state machine (one will be spawned per connection)
async fn handle_socket(
    mut socket: WebSocket,
    who: SocketAddr,
    state: Arc<AppState>,
    user: Option<User>,
) {
    // send a ping (unsupported by some browsers) just to kick things off and get a response
    if socket
        .send(Message::Ping(Bytes::from_static(&[1, 2, 3])))
        .await
        .is_ok()
    {
        debug!("Pinged {who}...");
    } else {
        debug!("Could not send ping {who}!");
        // no Error here since the only thing we can do is to close the connection.
        // If we can not send messages, there is no way to salvage the statemachine anyway.
        return;
    }

    // By splitting socket we can send and receive at the same time. In this example we will send
    // unsolicited messages to client based on some sort of server's internal event (i.e .timer).
    let (mut sender, mut receiver) = socket.split();

    let slot: crate::events::SlotMap<i32> = Arc::new(Mutex::new(HashMap::new()));
    let notify = Arc::new(Notify::new());

    let slot_send = Arc::clone(&slot);
    let notify_send = Arc::clone(&notify);
    let mut send_task = tokio::spawn(async move {
        loop {
            notify_send.notified().await;
            let pending: Vec<Arc<String>> = {
                let mut map = slot_send.lock().unwrap();
                map.drain().map(|(_, v)| v).collect()
            };
            for msg in pending {
                if sender
                    .send(Message::Text(msg.to_string().into()))
                    .await
                    .is_err()
                {
                    return 1;
                }
            }
        }
    });

    let slot_recv = Arc::clone(&slot);
    let notify_recv = Arc::clone(&notify);
    let mut socket_auth = user
        .map(|u| crate::events::Auth::Account(u.id))
        .unwrap_or(crate::events::Auth::Anonymous);
    let mut recv_task = tokio::spawn(async move {
        let mut subscribed: HashSet<(String, i32)> = HashSet::new();
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(msg_text) = msg
                && let Ok(frame) = serde_json::from_str::<WsFrame>(msg_text.as_str())
            {
                match frame {
                    WsFrame::Auth { auth_key } => {
                        // The key only counts before the first subscription
                        if subscribed.is_empty()
                            && state
                                .config
                                .bookables
                                .as_ref()
                                .and_then(|b| b.kiosk.as_ref())
                                .is_some_and(|k| k.matches(&auth_key))
                        {
                            socket_auth = crate::events::Auth::Kiosk;
                            debug!("kiosk authenticated");
                        }
                        continue;
                    }
                    WsFrame::Subscription { channel, id } => {
                        let id: i32 = id.into();
                        let key = (channel.clone(), id);
                        if subscribed.insert(key) {
                            match channel.as_str() {
                                "bookables" => {
                                    state
                                        .event_channels
                                        .bookables
                                        .subscribe(
                                            id,
                                            socket_auth.clone(),
                                            Arc::clone(&slot_recv),
                                            Arc::clone(&notify_recv),
                                        )
                                        .await;
                                }
                                "bookables/queues" => {
                                    state
                                        .event_channels
                                        .bookable_queues
                                        .subscribe(
                                            id,
                                            socket_auth.clone(),
                                            Arc::clone(&slot_recv),
                                            Arc::clone(&notify_recv),
                                        )
                                        .await;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    });

    // If any one of the tasks exit, abort the other.
    tokio::select! {
        rv_a = (&mut send_task) => {
            match rv_a {
                Ok(a) => debug!("{a} messages sent to {who}"),
                Err(a) => debug!("Error sending messages {a:?}")
            }
            recv_task.abort();
        },
        rv_b = (&mut recv_task) => {
            match rv_b {
                Ok(_) => debug!("Receiver task closed"),
                Err(b) => debug!("Error receiving messages {b:?}")
            }
            send_task.abort();
        }
    }

    // returning from the handler closes the websocket connection
    debug!("Websocket context {who} destroyed");
}
