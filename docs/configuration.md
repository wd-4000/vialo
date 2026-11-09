# Configuration

vialo-api and vialo-dhcp are configured through a single `vialo.toml` file and
environment variables.

Both find the file the same way (`vialo-common::load`):

1. `$VIALO_CONFIG`, if set, is used as an explicit path.
2. Otherwise the binary searches upward from its working directory for
   `vialo.toml`.

Equivalent process for .env. Secrets (`DATABASE_URL`, `ENCRYPTION_KEY`) stay there. 
`vialo.toml` itself has to be readable by the service user, and both units use
`DynamicUser=yes`, so mode 0644 root:root is the straightforward choice.
Keep secrets out of it!

## Address format

### Listeners

| Pattern | Transport |
|---|---|
| Starts with `/` | Filesystem Unix socket (`/run/vialo-api/public.sock`) |
| Starts with `@` | Abstract namespace Unix socket (`@vialo-api`) |
| Everything else | TCP (`0.0.0.0:8000`, `127.0.0.1:8011`) |

### Email

| Pattern | Transport |
|---|---|
| `smtp://emailuser:emailpass@127.0.0.1:1025/` | SMTP |
| `sendmail://` | Pipe mail through the local `/usr/sbin/sendmail` binary |

### Outbound connections
Kratos and UniFi hostnames support Unix sockets via a `unix://` prefix.

| Pattern | Transport |
|---|---|
| `http://localhost:4434` | You know |
| `unix:///run/kratos/admin.sock` | HTTP over a Unix socket |

## `vialo.toml`

Keys are documented in the config structs (`vialo-api/src/config.rs`,
`vialo-common/src/dhcp.rs`). For a JSON Schema of the whole file, with
those docs as descriptions:

```sh
cargo run -p vialo-api --bin config-schema --features migrate vialo.schema.json
```

`DATABASE_URL` comes from the environment. See
`vialo-dhcp/debian/dhcp.env.example` for vialo-dhcp's template. Each
service keeps its own env file so that the API's secrets stay separate.

## Environment variables

| Variable | Required | Description |
|---|---|---|
| `DATABASE_URL` | yes | PostgreSQL connection string. Supports Unix sockets via `host=/path/to/socket` or by omitting the host. Read by both binaries. |
| `VIALO_CONFIG` | no | Explicit path to `vialo.toml`, skipping the upward search. Read by both binaries. The systemd units set it to `/etc/vialo/vialo.toml`. |
| `ENCRYPTION_KEY` | in production | 32 bytes as 64 hex characters. Encrypts credentials and PPSK passwords at rest. In dev, a hardcoded default is used. |
| `ENCRYPTION_KEY_PATH` | alternative | Path to a file containing the encryption key. Mutually exclusive with `ENCRYPTION_KEY`. |
| `EMAIL_URL` | with `email` feature | SMTP URL (`smtp://host:25`) or `sendmail://` to use the local sendmail binary. |
| `PRINTER_URL` | with `printer` feature | Base URL of the Konica Minolta printer's OpenAPI endpoint. |
| `PRINTER_PASSWORD` | with `printer` feature | Admin password for the printer API. |
| `ORY_KRATOS_ADMIN_URL` | with Kratos auth | Kratos admin API base URL (used for identity deletion requests). Supports `unix://` prefix for Unix socket connections. |
| `INITIAL_ADMIN_EMAIL` | no | If set and no accounts exist yet, the first person with this email to sign up becomes an admin. |

## Feature flags (vialo-api)

The `vialo-api` crate has optional features that gate functionality at compile time.
The Dockerfiles build with `migrate,media-asm,media-heif-static` on top of the defaults.

| Feature | Default | What it gates |
|---|---|---|
| `email` | yes | Email notification subsystem (`lettre` + handlebars templates) and `[email]`. |
| `printer` | yes | Printer subsystem (job queue + printer API client). |
| `nms` | yes | Network management subsystem that pushes PPSK lists to UniFi and MikroTik connectors. |
| `migrate` | no | Runs `sqlx::migrate!()` on startup. You want this unless running migrations separately. Also gates `[dev]`. |
| `import` | no | Building blocks for legacy-data importers. |
| `media-asm` | no | Assembly for the AVIF encoder (rav1e). Needs `nasm` at build time. |
| `media-heif` | no | HEIF/AVIF uploads. Links the system `libheif`. |
| `media-heif-static` | no | `media-heif` with libheif built from source and linked statically (Docker/musl). Needs cmake, g++ and static libde265 + libaom. |
