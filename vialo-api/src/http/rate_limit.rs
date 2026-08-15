use std::{
    net::{IpAddr, SocketAddr},
    num::NonZeroU32,
    sync::Arc,
};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use governor::{
    DefaultKeyedRateLimiter, Quota, RateLimiter,
    clock::{Clock, DefaultClock},
};
use serde_json::json;
use uuid::Uuid;

use crate::{AppState, health::add_health_event, http::history::models::Subsystem};

use super::util::{User, VialoError};

type RlInstant = <DefaultClock as Clock>::Instant;

const STRIKE_THRESHOLD: u32 = 10;

pub struct RateLimiters {
    pub global: Arc<DefaultKeyedRateLimiter<IpAddr>>,
    pub authenticated: Arc<DefaultKeyedRateLimiter<Uuid>>,
    pub credential: Arc<DefaultKeyedRateLimiter<Uuid>>,
    pub credits: Arc<DefaultKeyedRateLimiter<Uuid>>,
    /// Separate IP-keyed limiter for the unauthenticated fallback path.
    /// Uses its own bucket so it doesn't double-consume from `global`.
    pub anonymous: Arc<DefaultKeyedRateLimiter<IpAddr>>,
    /// IP-keyed limiter for kiosk PIN attempts. Checked in the activate handler
    /// (kiosk branch only) rather than as middleware, because the endpoint is
    /// shared with the session flow.
    pub kiosk_pin: Arc<DefaultKeyedRateLimiter<IpAddr>>,
    pub strikes: Arc<DashMap<IpAddr, u32>>,
}

impl Default for RateLimiters {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiters {
    pub fn new() -> Self {
        let per_min = |n: u32| Quota::per_minute(NonZeroU32::new(n).unwrap());
        Self {
            global: Arc::new(RateLimiter::keyed(per_min(120))),
            authenticated: Arc::new(RateLimiter::keyed(per_min(200))),
            credential: Arc::new(RateLimiter::keyed(per_min(10))),
            credits: Arc::new(RateLimiter::keyed(per_min(5))),
            anonymous: Arc::new(RateLimiter::keyed(per_min(120))),
            kiosk_pin: Arc::new(RateLimiter::keyed(per_min(10))),
            strikes: Arc::new(DashMap::new()),
        }
    }

    /// In-handler check for kiosk PIN attempts keyed by client IP
    pub fn check_kiosk_pin(
        &self,
        headers: &HeaderMap,
        connect_info: Option<&ConnectInfo<SocketAddr>>,
    ) -> Result<(), VialoError> {
        let ip = extract_ip(headers, connect_info);
        self.kiosk_pin.check_key(&ip).map_err(|_| {
            VialoError::AppError(StatusCode::TOO_MANY_REQUESTS, "too_many_requests".into())
        })
    }
}

pub fn extract_ip(headers: &HeaderMap, connect_info: Option<&ConnectInfo<SocketAddr>>) -> IpAddr {
    headers
        .get("X-Forwarded-For")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse().ok())
        .or_else(|| connect_info.map(|ci| ci.0.ip()))
        .unwrap_or(IpAddr::from([127, 0, 0, 1]))
}

fn reject(not_until: governor::NotUntil<RlInstant>) -> Response {
    let secs = not_until
        .wait_time_from(DefaultClock::default().now())
        .as_secs()
        + 1;
    (
        StatusCode::TOO_MANY_REQUESTS,
        [("Retry-After", secs.to_string())],
        "Too many requests",
    )
        .into_response()
}

fn check(
    user_limiter: &DefaultKeyedRateLimiter<Uuid>,
    ip_limiter: &DefaultKeyedRateLimiter<IpAddr>,
    user_id: Option<Uuid>,
    ip: IpAddr,
) -> Result<(), governor::NotUntil<RlInstant>> {
    match user_id {
        Some(uid) => user_limiter.check_key(&uid),
        None => ip_limiter.check_key(&ip),
    }
}

fn keys(request: &Request) -> (Option<Uuid>, IpAddr) {
    let user_id = request.extensions().get::<User>().map(|u| u.id);
    let ip = extract_ip(
        request.headers(),
        request.extensions().get::<ConnectInfo<SocketAddr>>(),
    );
    (user_id, ip)
}

/// Increments the per-IP strike counter and fires a health event at the threshold.
fn record_strike(state: &Arc<AppState>, user_id: Option<Uuid>, ip: IpAddr) {
    let count = {
        let mut entry = state.rate_limiters.strikes.entry(ip).or_insert(0);
        *entry += 1;
        *entry
    };
    if count >= STRIKE_THRESHOLD {
        let db = state.db.clone();
        let ip_str = ip.to_string();
        tokio::spawn(async move {
            add_health_event(
                &db,
                Subsystem::App,
                "rate_limit_strikes",
                Some(json!({
                    "ip": ip_str,
                    "user_id": user_id,
                    "consecutive_blocks": count,
                })),
                20,
                false,
                Some(&ip_str),
            )
            .await;
        });
    }
}

/// Observer: tracks consecutive 429s per IP and fires a health event at the threshold.
/// Placed as a route_layer inside auth_middleware so the User extension is populated.
/// Captures strikes from inner rate-limit middlewares (authenticated, credential, credits).
pub async fn observe(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    let (user_id, ip) = keys(&request);
    let response = next.run(request).await;
    if response.status() == StatusCode::TOO_MANY_REQUESTS {
        record_strike(&state, user_id, ip);
    } else if response.status().is_success() {
        // Reset counter to 0 instead of removing the entry.
        // Avoids a race where concurrent remove() erases a just-incremented counter.
        if let Some(mut entry) = state.rate_limiters.strikes.get_mut(&ip) {
            *entry = 0;
        }
    }
    response
}

/// Global IP-keyed limit applied to all routes (120/min).
/// Also records strikes for 429s so the observer isn't blind to outer-limiter blocks.
pub async fn global(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    let ip = extract_ip(
        request.headers(),
        request.extensions().get::<ConnectInfo<SocketAddr>>(),
    );
    match state.rate_limiters.global.check_key(&ip) {
        Ok(_) => next.run(request).await,
        Err(not_until) => {
            // Track the strike — user_id is not available this early in the chain
            record_strike(&state, None, ip);
            reject(not_until)
        }
    }
}

/// User-keyed limit for authenticated routes (200/min), falls back to anonymous IP limit.
pub async fn authenticated(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let (user_id, ip) = keys(&request);
    match check(
        &state.rate_limiters.authenticated,
        &state.rate_limiters.anonymous,
        user_id,
        ip,
    ) {
        Ok(_) => next.run(request).await,
        Err(not_until) => reject(not_until),
    }
}

/// Tight user-keyed limit for endpoints returning credentials (10/min).
pub async fn credential(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let (user_id, ip) = keys(&request);
    match check(
        &state.rate_limiters.credential,
        &state.rate_limiters.anonymous,
        user_id,
        ip,
    ) {
        Ok(_) => next.run(request).await,
        Err(not_until) => reject(not_until),
    }
}

/// Tight user-keyed limit for credits operations (5/min).
pub async fn credits(State(state): State<Arc<AppState>>, request: Request, next: Next) -> Response {
    let (user_id, ip) = keys(&request);
    match check(
        &state.rate_limiters.credits,
        &state.rate_limiters.anonymous,
        user_id,
        ip,
    ) {
        Ok(_) => next.run(request).await,
        Err(not_until) => reject(not_until),
    }
}
