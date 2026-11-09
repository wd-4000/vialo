# Architecture

`vialo-api` holds the API and its background subsystems. `vialo-dhcp`
is a separate crate in the same workspace. In `vialo-api/src`, `http/` has the
public API and `hooks/` the internal one. The other folders are mostly one
subsystem each.

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

## Subsystems

Subsystems are long-running tasks inside the `vialo-api` process, started through
`run_subsystem` in `main.rs`. If one exits unexpectedly or panics, it restarts
after 5 seconds unless the server is shutting down.

They don't all get their work the same way:

| Subsystem | Code | Work comes from |
|---|---|---|
| Printer | `printer::main()` | `subsystem_jobs` table |
| NMS | `nms::main()` | `subsystem_jobs` table |
| Media | `media::main()` | `subsystem_jobs` table |
| Email | `email::main()` | Broadcast channels (`posts_tx`, `bookable_events_tx`) |
| Bookables | `bookables::main()` | Postgres `LISTEN`/`NOTIFY` |
| Memberships | `memberships::main()` | Timer, at midnight |

Jobs in `subsystem_jobs` move through `pending` → `processing` → `done` or
`error`. They live in the database, so they survive a restart.

DHCP is the exception: `vialo-dhcp` is a standalone binary with its own process
lifecycle, using dora for the DHCP protocol.

## Auth

### Mock auth (development)
A hardcoded UUID from `vialo.toml` is always authenticated. An account for it is
created on first start.

### Kratos auth (production)
1. The auth middleware (`http/util/middleware.rs`) reads the session token from
   the `X-Session-Token` header, or falls back to the `Cookie` header.
2. It calls the Kratos `to_session` API to validate the session.
3. It finds the account whose `auth_id` matches the identity. This is **not** the
   account's own `id`.
4. It inserts a `User` extension into the request, for handlers and the rate
   limiter. A suspended account also gets an `AuthError` extension.

### Identity lifecycle
- Kratos POSTs `/hooks/update_identity` when an identity is created or updated.
  The handler upserts into the `identities` table and can bootstrap the first
  admin account.
- `helpers::people::delete_identity` removes the local row and, if
  `ORY_KRATOS_ADMIN_URL` is set, the Kratos identity. Callers run it inside their
  transaction, so if Kratos fails, the local delete rolls back too.

## Database session variables

Two session variables record who is making a change:

- `app.account_id`: set by `grab_authd_conn_user()` for user actions.
- `app.subsystem`: set by `grab_authd_conn_subsystem()` for subsystem actions.

The `table_log` trigger reads them to attribute changes in the audit log, and
**rejects writes** to logged tables when neither is set. They are for attribution
only. There is no row-level security, so access checks belong in the handlers.

They are reset when a connection returns to the pool (`after_release` in
`main.rs`). Handlers get a transaction from one of these connections with
`grab_trans()`.
