# Architecture

How the vialo-api code is organized and how the pieces fit together.

## Crate map

```
src/
├── main.rs           # Entry point: config loading, pool setup, listener binding,
│                       subsystem spawning, graceful shutdown
├── lib.rs            # AppState, KratosConfigs, EventChannels, shared macros
├── config.rs         # Config deserialization (vialo.toml → Config struct)
├── http/             # Public API routes, handlers, middleware, rate limiting
│   ├── mod.rs        # create_router(): assembles all route groups
│   ├── util/         # Auth middleware (mock + Kratos), pagination, transactions
│   ├── rate_limit.rs # Per-user and per-IP rate limiting (governor)
│   ├── people/       # Account management
│   ├── network/      # Networks, realms, devices, credentials, NMS connectors
│   ├── bookables/    # Asset types, assets, appointments, connectors
│   ├── posts/        # Boards, posts, subscriptions
│   ├── home/         # Dashboard quicklinks
│   ├── history/      # Audit log / health events
│   ├── health/       # Health check endpoint
│   └── docs.rs       # OpenAPI (utoipa) spec generation
├── hooks/            # Hooks API. Unauthenticated webhook handlers.
│   ├── mod.rs        # create_router(), /hooks/update_identity
│   └── radius.rs     # /radius/auth/{network_id}: FreeRADIUS auth endpoint
├── ws.rs             # WebSocket server for real-time event subscriptions
├── bookables/        # Bookable connector subsystem (NetIO smart outlets)
├── printer/          # Printer subsystem (Konica Minolta API)
├── email/            # Email subsystem (SMTP/sendmail + handlebars templates)
├── ppsk/             # PPSK subsystem (UniFi controller credential sync)
├── helpers/          # Encryption, people helpers, file utilities
├── permissions.rs    # App role checks
├── events.rs         # Broadcast channel wiring
├── health.rs         # Health event recording
└── dump.rs           # Database dump utility
```

`vialo-dhcp/` is a separate crate in the same workspace:

```
vialo-dhcp/
├── main.rs           # CLI parsing (clap), pool setup, dora Server bootstrap
├── lib.rs            # VialoDhcp plugin: discover, request, release, decline handlers
└── tests.rs          # Integration tests using sqlx::test
```

## Public & Hooks APIs

vialo-api listens on **two separate addresses** (TCP ports or Unix sockets):

| API | Listener config | Auth | CORS | Rate limiting | Purpose |
|---|---|---|---|---|---|
| Public | `[public].listen` | Kratos sessions or mock | Yes | Yes | Browser UI, admin panel |
| Hooks | `[hooks].listen` | **None** | No | No | Kratos webhooks, RADIUS auth |

The split is a security boundary. The hooks API has no authentication because
its callers (Kratos, FreeRADIUS) are trusted internal services. It is meant to
be bound to `127.0.0.1` or a Unix socket. It must never be exposed to the
network.

In `main.rs`, the two routers are created separately (`create_router` for
public, `create_router` for hooks) and served concurrently via `tokio::join!`.
The public router also merges in the WebSocket handler and a CORS layer.

## Subsystem model

Most subsystems run as long‑running async tasks inside the `vialo-api` process.
DHCP is the exception: it is a standalone binary with its own process lifecycle.
The in‑process subsystems follow the same pattern:

1. Spawned as a `tokio::spawn` or `LocalSet::run_until` in `main.rs`.
2. Loops on a job queue in the `subsystem_jobs` table (status: `pending` →
   `processing` → `done` / `error`).
3. Restarts itself on unexpected exit (delayed retry, respect shutdown signal).
4. Records health events on errors (visible in the admin health dashboard).

| Subsystem | Code | Job data |
|---|---|---|
| Printer | `printer::main()` | CreateAccount, DeleteAccount, UpdateAccountLimit, Refresh, FullSync |
| PPSK | `ppsk::main()` | Refresh (sync credentials to UniFi) |
| Email | `email::main()` | Event‑driven via broadcast channels, no job table |
| Bookables | `bookables::main()` | Connector power on/off on appointment start/end |
| DHCP | `vialo-dhcp` binary | Discover, Request, Release, Decline. Standalone process using dora for the DHCP protocol. |

Email is the exception. It listens on `tokio::sync::broadcast` channels
(`posts_tx`, `expired_appointments_tx`) instead of polling a job table.

## Auth flow

### Mock auth (development)
A hardcoded UUID from `vialo.toml` is always authenticated. An admin account
is auto‑created on first start. No external services needed.

### Kratos auth (production)
1. The auth middleware (`http/util/middleware.rs`) extracts the session token
   from the `X-Session-Token` header, or falls back to the `Cookie` header.
2. It calls the Kratos `to_session` API directly to validate the session.
3. On success, a `User` extension is inserted into the request with the
   account UUID looked up from the identity's `auth_id`.
4. Downstream handlers and the rate limiter read the `User` extension.

### Identity lifecycle
- Kratos POSTs `/hooks/update_identity` when an identity is created or updated.
- The hook handler upserts into the `identities` table and optionally creates
  the first admin account.
- When an account is deleted, `helpers/people.rs` calls the Kratos admin API to
  delete the identity as well. This is best‑effort. The local delete proceeds
  even if Kratos is unreachable.

## Database patterns

### Session variables
Two PostgreSQL session variables scope database access:

- `app.account_id`: set by `grab_authd_conn_subsystem()` for user‑scoped
  operations. Row‑level security policies filter on this.
- `app.subsystem`: set by the same function for subsystem‑scoped operations
  (printer, PPSK, etc.).

These are reset on connection release (`after_release` hook in the pool).

### Transactions
Handlers use `grab_trans()` to get a transaction from an auth'd connection.
This ensures the session variables are set before any application queries run.

### Compiled‑checked queries
All SQL uses `sqlx::query!()` and `sqlx::query_scalar!()` macros, which verify
queries against the database at compile time. No runtime query building.

## Rate limiting

The rate limiter (`http/rate_limit.rs`) applies per‑route limits using the
`governor` crate:

- **Authenticated requests**: rate limited by user UUID.
- **Anonymous requests**: rate limited by client IP. The IP comes from the
  `X-Forwarded-For` header, or the `ConnectInfo` peer address, or falls back
  to `127.0.0.1`.

Different endpoint groups have different limits. Credential endpoints are
stricter than general read endpoints. Rate limit state is in‑memory
(per‑process).

## WebSocket

The WebSocket endpoint (`/ws` on the public API) provides real‑time event
subscriptions. Clients send JSON messages to subscribe to channels (currently
`bookables`). The server pushes updates when subscribed resources change.

`EventChannels` in `AppState` holds the broadcast senders. The bookables
channel uses a custom `BookableChannel` that manages per‑resource
subscriptions.
