//! WebSocket server.
//! Handles event subscriptions

use axum::{
    Router,
    body::Bytes,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::any,
};
use axum_extra::TypedHeader;
use serde::Deserialize;
use std::net::SocketAddr;
use std::{collections::HashSet, sync::Arc};
use tokio::sync::mpsc::{self};
use tower_http::trace::{DefaultMakeSpan, TraceLayer};
use tracing::debug;

//allows to extract the IP of connecting user
use axum::extract::connect_info::ConnectInfo;

//allows to split the websocket stream into separate TX and RX branches
use crate::AppState;
use futures::{sink::SinkExt, stream::StreamExt};
#[derive(Deserialize, Debug)]
struct EventSubscriptionSchema {
    pub channel: String,
    pub id: u16,
}
pub fn main(app_state: Arc<AppState>) -> Router {
    // build our application with some routes

    Router::new()
        .route("/ws", any(ws_handler))
        // logging so we can see what's going on
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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    let user_agent = if let Some(TypedHeader(user_agent)) = user_agent {
        user_agent.to_string()
    } else {
        String::from("Unknown browser")
    };
    debug!("`{user_agent}` at {addr} connected.");
    // finalize the upgrade process by returning upgrade callback.
    // we can customize the callback by sending additional info such as address.
    ws.on_upgrade(move |socket| handle_socket(socket, addr, state))
}

/// Actual websocket state machine (one will be spawned per connection)
async fn handle_socket(mut socket: WebSocket, who: SocketAddr, state: Arc<AppState>) {
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
    let (mpsc_tx, mut mpsc_rx) = mpsc::channel::<Arc<String>>(10);
    // Spawn a task that will push several messages to the client (does not matter what client does)
    let mut send_task = tokio::spawn(async move {
        loop {
            if let Some(msg) = mpsc_rx.recv().await {
                // In case of any websocket error, we exit.
                if sender
                    .send(Message::Text(msg.to_string().into()))
                    .await
                    .is_err()
                {
                    return 1;
                }
            } else {
                return 0;
            }
        }

        // debug!("Sending close to {who}...");
        // if let Err(e) = sender
        //     .send(Message::Close(Some(CloseFrame {
        //         code: axum::extract::ws::close_code::NORMAL,
        //         reason: Utf8Bytes::from_static("Goodbye"),
        //     })))
        //     .await
        // {
        //     debug!("Could not send Close due to {e}, probably it is ok?");
        // }
        // n_msg
    });

    // This task receives messages from the client and subscribes it to events based on the message contents
    let mut recv_task = tokio::spawn(async move {
        let mut subscribed: HashSet<(String, i32)> = HashSet::new();
        while let Some(Ok(msg)) = receiver.next().await {
            if let Message::Text(msg_text) = msg
                && let Ok(sub) = serde_json::from_str::<EventSubscriptionSchema>(msg_text.as_str())
            {
                let id: i32 = sub.id.into();
                let key = (sub.channel.clone(), id);
                if subscribed.insert(key) {
                    if sub.channel.as_str() == "bookables" {
                        state
                            .event_channels
                            .bookables
                            .subscribe_string(id, mpsc_tx.clone())
                            .await
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
