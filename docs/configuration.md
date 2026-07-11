# Configuration

vialo-api is configured through a `vialo.toml` file and environment variables.
At startup the app searches upward from the working directory for `vialo.toml`
(using `find_upward`).

## Address format

The `listen` fields and outbound connection URLs use the address string itself
to determine transport:

| Pattern | Transport |
|---|---|
| Starts with `/` | Filesystem Unix socket (`/run/vialo-api/public.sock`) |
| Starts with `@` | Abstract namespace Unix socket (`@vialo-api`) |
| Everything else | TCP (`0.0.0.0:8000`, `127.0.0.1:8011`) |

`EMAIL_URL` adds one more pattern:

| Pattern | Transport |
|---|---|
| `sendmail://` | Pipe mail through the local `/usr/sbin/sendmail` binary |

Kratos and UniFi hostnames support Unix sockets via a `unix://` prefix:

| Pattern | Transport |
|---|---|
| `unix:///run/kratos/admin.sock` | HTTP over a Unix socket (reqwest `unix_socket`) |

## `vialo.toml`

```toml
[public]
cors_origins = ["http://localhost:3000", "http://localhost:3001"]
listen = "0.0.0.0:8000"

[hooks]
listen = "0.0.0.0:8011"

[auth]
type = "mock"       # or "kratos"
# … variant-specific fields below

[org]
name = "My Organization"
domain = "example.com"
short_name = "MYORG"
impressum = "https://example.com/impressum"

[proxy]
# Optional SOCKS5 proxy for outbound HTTP requests (printer, bookable connectors).
# Omit the entire [proxy] section if you don't need one.
# proxy = "socks5://127.0.0.1:1080"

[email]             # requires feature `email`
[email.url]
unsubscribe = "https://example.com/unsubscribe/"
post = "https://example.com/posts/"
board = "https://example.com/boards/"
preferences = "https://example.com/preferences/"
```

### `[public]`

| Key | Required | Description |
|---|---|---|
| `listen` | yes | Listen address for the browser-facing API. Supports TCP and Unix (see [address format](#address-format)). |
| `cors_origins` | yes | List of allowed CORS origins. The frontend URLs that call this API. |

### `[hooks]`

| Key | Required | Description |
|---|---|---|
| `listen` | yes | Listen address for webhooks (Kratos identity sync, RADIUS auth). **No authentication**. Bind to `127.0.0.1` or a Unix socket. |

### `[auth]`

Tagged enum. The `type` field selects the variant:

**`type = "mock"`**

| Key | Required | Description |
|---|---|---|
| `uuid` | yes | A hardcoded UUID that always authenticates as admin. |
| `email` | yes | Email address for the auto-created admin account. |

Use this for development. A mock admin account is created automatically on first
start if it doesn't already exist.

**`type = "kratos"`**

| Key | Required | Description |
|---|---|---|
| `frontend_url` | yes | Kratos public API (for session validation). |
| `admin_url` | yes | Kratos admin API (for identity deletion). |

When using Kratos, set `ORY_KRATOS_ADMIN_URL` in the environment as well. It is
used separately from the config file for direct HTTP calls.

### `[org]`

| Key | Required | Description |
|---|---|---|
| `name` | yes | Full organization name. |
| `domain` | yes | Domain for generated email addresses (`noreply@domain`). |
| `short_name` | yes | Abbreviation used in email subjects and templates. |
| `impressum` | yes | URL to the legal notice / Impressum page. |

### `[proxy]` (optional)

| Key | Required | Description |
|---|---|---|
| `proxy` | no | SOCKS5 proxy URL for outbound HTTP clients (printer, bookable connectors). If absent, no proxy is used. |

Note: the PPSK/UniFi subsystem also respects this setting.

### `[email]` (requires feature `email`)

| Key | Required | Description |
|---|---|---|
| `email.url.unsubscribe` | yes | Base URL for one-click unsubscribe links. |
| `email.url.post` | yes | Base URL for links to individual posts. |
| `email.url.board` | yes | Base URL for links to boards. |
| `email.url.preferences` | yes | Base URL for email preference management. |

## Environment variables

| Variable | Required | Description |
|---|---|---|
| `DATABASE_URL` | yes | PostgreSQL connection string. Supports Unix sockets via `host=/path/to/socket` or by omitting the host. |
| `ENCRYPTION_KEY` | in production | 32 bytes as 64 hex characters. Encrypts credentials and PPSK passwords at rest. In dev, a hardcoded default is used. |
| `ENCRYPTION_KEY_PATH` | alternative | Path to a file containing the encryption key. Mutually exclusive with `ENCRYPTION_KEY`. |
| `EMAIL_URL` | with `email` feature | SMTP URL (`smtp://host:25`) or `sendmail://` to use the local sendmail binary. |
| `PRINTER_URL` | with `printer` feature | Base URL of the Konica Minolta printer's OpenAPI endpoint. |
| `PRINTER_PASSWORD` | with `printer` feature | Admin password for the printer API. |
| `ORY_KRATOS_ADMIN_URL` | with Kratos auth | Kratos admin API base URL (used for identity deletion requests). Supports `unix://` prefix for Unix socket connections. |
| `INITIAL_ADMIN_EMAIL` | no | If set and no accounts exist yet, the first person with this email to sign up becomes an admin. |

## Feature flags

## DHCP server (`vialo-dhcp`)

The DHCP server is a standalone binary with its own CLI. All options can also
be set via environment variables (uppercase, hyphens replaced with underscores).

| Flag | Env | Default | Description |
|---|---|---|---|
| `--database-url` | `DATABASE_URL` | (required) | PostgreSQL connection string. |
| `--listen` | `LISTEN` | `0.0.0.0:67` | DHCP listen address (UDP). |
| `--siaddr` | `SIADDR` | (required) | Server identifier IP. Must be an IP on the listen interface. |
| `--interfaces` | `INTERFACES` | all | Comma-separated interface names to bind. Empty binds all. |
| `--lease-time` | `LEASE_TIME` | `10800` | Lease duration in seconds. T1 = lease/2, T2 = lease × 7/8. |
| `--probation` | `PROBATION` | `3600` | How long a declined address stays quarantined, in seconds. |
| `--circuit-id-vlan` | `CIRCUIT_ID_VLAN` | `ascii` | Option 82 Circuit ID VLAN parsing: `off`, `ascii`, `binary`. |
| `--pg-max-connections` | `PG_MAX_CONNECTIONS` | `5` | Maximum Postgres connection pool size. |

See `vialo/vialo-dhcp/debian/dhcp.env.example` for a template.

## Feature flags (vialo-api)

The `vialo-api` crate has optional features that gate functionality at compile time.
All are enabled by default.

| Feature | What it gates |
|---|---|
| `email` | Email notification subsystem (`lettre` + handlebars templates). |
| `printer` | Printer subsystem (job queue + KM printer API client). |
| `printer_km` | Konica Minolta printer backend. Requires `printer`. |
| `ppsk` | UniFi PPSK credential push subsystem. |
| `migrate` | Runs `sqlx::migrate!()` on startup. You want this unless running migrations separately. |
