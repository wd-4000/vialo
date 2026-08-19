# Configuration

vialo-api and vialo-dhcp are configured through a single `vialo.toml` file and
environment variables. Each binary reads only the sections it needs and ignores
the rest, so one file serves the whole stack.

Both find the file the same way (`vialo-common::load`):

1. `$VIALO_CONFIG`, if set, is used as an explicit path.
2. Otherwise the binary searches upward from its working directory for
   `vialo.toml`.

Services start in `/`, so the upward search would only ever find `/vialo.toml`.
Both systemd units therefore set `VIALO_CONFIG=/etc/vialo/vialo.toml`
themselves; the env files can override it, but normally don't mention it.

Secrets (`DATABASE_URL`, `ENCRYPTION_KEY`) stay in the environment files, which
are read by PID 1 at mode 0600. `vialo.toml` itself has to be readable by the
service user, and both units use `DynamicUser=yes`, so mode 0644 root:root is
the straightforward choice — keep secrets out of it.

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

[dhcp]              # read by vialo-dhcp, ignored by vialo-api
siaddr = "192.0.2.1"
interfaces = ["eth0"]
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

### `[dhcp]` (read by `vialo-dhcp`)

Unknown keys in this section are rejected rather than ignored: a typo would
otherwise silently leave the default in place.

| Key | Required | Default | Description |
|---|---|---|---|
| `siaddr` | yes | — | Server identifier IP. Must be an IP on the listen interface. |
| `listen` | no | `0.0.0.0:67` | DHCP listen address (UDP). |
| `interfaces` | no | `[]` | Interface names to bind. Empty binds all; pin a single one in production, which is what enables `SO_BINDTODEVICE`. |
| `lease_time` | no | `10800` | Lease duration in seconds. T1 = lease/2, T2 = lease × 7/8. |
| `probation` | no | `3600` | How long a declined address stays quarantined, in seconds. |
| `circuit_id_vlan` | no | `ascii` | Option 82 Circuit ID VLAN parsing: `off`, `ascii`, `binary`. |
| `pg_max_connections` | no | `5` | Maximum Postgres connection pool size. |

`DATABASE_URL` comes from the environment — see
`vialo/vialo-dhcp/debian/dhcp.env.example` for a template. Each service keeps
its own env file so that the API's secrets stay out of the environment of the
process parsing packets off the wire.

## Environment variables

| Variable | Required | Description |
|---|---|---|
| `DATABASE_URL` | yes | PostgreSQL connection string. Supports Unix sockets via `host=/path/to/socket` or by omitting the host. Read by both binaries. |
| `VIALO_CONFIG` | no | Explicit path to `vialo.toml`, skipping the upward search. Read by both binaries; the systemd units set it to `/etc/vialo/vialo.toml`. |
| `ENCRYPTION_KEY` | in production | 32 bytes as 64 hex characters. Encrypts credentials and PPSK passwords at rest. In dev, a hardcoded default is used. |
| `ENCRYPTION_KEY_PATH` | alternative | Path to a file containing the encryption key. Mutually exclusive with `ENCRYPTION_KEY`. |
| `EMAIL_URL` | with `email` feature | SMTP URL (`smtp://host:25`) or `sendmail://` to use the local sendmail binary. |
| `PRINTER_URL` | with `printer` feature | Base URL of the Konica Minolta printer's OpenAPI endpoint. |
| `PRINTER_PASSWORD` | with `printer` feature | Admin password for the printer API. |
| `ORY_KRATOS_ADMIN_URL` | with Kratos auth | Kratos admin API base URL (used for identity deletion requests). Supports `unix://` prefix for Unix socket connections. |
| `INITIAL_ADMIN_EMAIL` | no | If set and no accounts exist yet, the first person with this email to sign up becomes an admin. |

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
