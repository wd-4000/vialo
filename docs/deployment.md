# Deployment

This guide covers deploying vialo-api in production on a Linux server with systemd.

## PostgreSQL

vialo requires **PostgreSQL 18 or later**.

The connection string lives in `/etc/vialo/api.env` (or wherever `DATABASE_URL`
points). For best performance and security, use a Unix socket:

```
DATABASE_URL=postgresql://user:pass@/vialo?host=/var/run/postgresql
```

For TCP:

```
DATABASE_URL=postgresql://user:pass@localhost:5432/vialo
```

Run migrations before starting the service for the first time (or enable the
`migrate` feature to run them automatically on startup):

```sh
cargo sqlx migrate run
```

## systemd

The Debian packaging (`cargo-deb`) installs two units:

### Service unit (`vialo-api.service`)

- `Type=exec`, runs under `DynamicUser=yes` with no capabilities
- Sandboxed: `ProtectSystem=strict`, `PrivateDevices=yes`, `NoNewPrivileges=yes`
- Only `AF_UNIX AF_INET AF_INET6` address families allowed
- Reads `/etc/vialo/api.env` for environment variables
- `ConfigurationDirectory=vialo` and `LogsDirectory=vialo-api` are the only
  writable paths

### Socket unit (`vialo-api.socket`)

Optional socket activation. When enabled, systemd creates the listening sockets
and passes them to the service. The service detects `LISTEN_FDS` on startup and
uses the pre‑bound sockets instead of binding its own.

```ini
[Socket]
ListenStream=/run/vialo-api/public.sock
ListenStream=/run/vialo-api/hooks.sock
FileDescriptorName=public
FileDescriptorName=hooks
SocketMode=0660
RuntimeDirectory=vialo-api
```

To use socket activation:

```sh
systemctl enable vialo-api.socket
systemctl start vialo-api.socket
```

The service starts on first connection. To use it without socket activation
(direct binding), start the service directly. It falls back to manual binding.

### Listen addresses

Configure `[public].listen` and `[hooks].listen` in `vialo.toml`:

| Use case | Config |
|---|---|
| Direct TCP | `listen = "127.0.0.1:8000"` |
| Direct Unix socket | `listen = "/run/vialo-api/public.sock"` |
| Socket activation | The `.socket` unit provides the sockets; `vialo.toml` is ignored for binding |

With Unix sockets, the hooks API is only reachable by processes with filesystem
access to the socket. No network exposure.

## Reverse proxy

Place nginx (or another TLS-terminating proxy) in front of the public API:

```nginx
server {
    listen 443 ssl;
    server_name vialo.example.com;

    location / {
        proxy_pass http://unix:/run/vialo-api/public.sock;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto https;
    }
}
```

The `X-Forwarded-For` header is used by the rate limiter to identify client IPs
through the proxy.

The hooks API should **not** be exposed through the reverse proxy. It has no
authentication and should only be reachable by Kratos and FreeRADIUS on a
local address or Unix socket.

## Kratos integration

vialo integrates with [Ory Kratos](https://www.ory.sh/kratos/) for identity
management:

1. Kratos handles user registration, login, and session management.
2. When an identity is created or updated, Kratos POSTs to
   `/hooks/update_identity` on the **hooks API**.
3. When an account is deleted, vialo calls the Kratos admin API to delete the
   identity (`DELETE /admin/identities/{id}`).

Configure the webhook in Kratos to point at the hooks listener:

```
# In your Kratos config: the base URL of the hooks API
webhooks:
  - url: http://127.0.0.1:8011/hooks/update_identity
```

If both vialo and Kratos are on the same host, use a Unix socket for the hooks
listener and the Kratos admin API connection:

```toml
# vialo.toml
[hooks]
listen = "/run/vialo-api/hooks.sock"
```

```
# .env
ORY_KRATOS_ADMIN_URL=unix:///run/kratos/admin.sock
```

## Printer subsystem

Requires the `printer` and `printer_km` features (enabled by default).

Set these environment variables:

```sh
PRINTER_URL=http://192.168.1.50:50001/OpenAPI
PRINTER_PASSWORD=your-admin-password
```

The printer subsystem runs as a background task. It:

- Polls the `subsystem_jobs` table for pending printer tasks
- Syncs user accounts between vialo and the printer
- Reads page counters and deducts credits
- Handles account creation, deletion, and limit changes

## PPSK subsystem

Requires the `ppsk` feature (enabled by default).

The PPSK subsystem pushes per‑account Wi‑Fi credentials to a UniFi controller.
It reads credentials from the `net_cred` table and the UniFi controller
hostname from `net_nms_connectors` (type `unifi`).

For an outbound proxy (e.g., SOCKS5 to reach the UniFi controller):

```toml
[proxy]
proxy = "socks5://127.0.0.1:1080"
```

To reach the UniFi controller through a Unix socket proxy:

```
# In net_nms_connectors.hostname:
unix:///run/unifi-proxy.sock
```

## DHCP server

The DHCP server (`vialo-dhcp`) is a standalone binary built on the
[dora](https://github.com/bluecatengineering/dora) DHCP library. It answers
DHCP for devices registered in the vialo database, handing out leases from
realm IP pools.

### Capabilities explanation

- `CAP_NET_BIND_SERVICE` to bind UDP port 67
- `CAP_NET_RAW` for `SO_BINDTODEVICE` when pinning to a single interface
- `AF_NETLINK` for VLAN detection via `rtnetlink`
- `SocketBindAllow=ipv4:udp:67`, `SocketBindDeny=any`

The systemd unit at `vialo/vialo-dhcp/debian/service` applies the same
sandboxing as vialo-api (`DynamicUser=yes`, `ProtectSystem=strict`, etc.).

### Env

Copy `vialo/vialo-dhcp/debian/dhcp.env.example` to `/etc/vialo/dhcp.env`.
`SIADDR` must be set to an IP address on the listen interface.

The DHCP server shares the same Postgres database as vialo-api.

### VLAN scoping

The server reads MAC addresses and VLAN hints to look up devices in the
`net_device_info` view. VLANs come from one of two sources, in priority order:

1. **Option 82 Agent Circuit ID**: parsed from the DHCP relay agent, format
   configured via `CIRCUIT_ID_VLAN` (`ascii` for Cisco-style, `binary` for a
   2-byte tag, or `off` to ignore).
2. **Interface**: for packets arriving on a VLAN subinterface, the kernel
   netlink interface provides the VLAN ID.
