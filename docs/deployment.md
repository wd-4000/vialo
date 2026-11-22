# Deployment

This guide covers deploying vialo-api in production on a Linux server with systemd.
Alternatively, there is a docker compose overlay (`compose.prod.yaml` in the repository root).
You can run it with `docker compose -f compose.yaml -f compose.prod.yaml up -d`.

## PostgreSQL

vialo requires **PostgreSQL 18 or later**. Debian's own package lags behind,
so install the latest from the [PGDG repository](https://wiki.postgresql.org/wiki/Apt):

```sh
apt install postgresql-common
/usr/share/postgresql-common/pgdg/apt.postgresql.org.sh
apt install postgresql
```

Then create the roles and databases:

```sh
sudo vialo db init
```

It creates the `vialo-api` role with the `vialo` database and `vialo-kratos`
with `kratos`, and installs `pg_stat_statements`. 

The services connect over `/run/postgresql`, so peer auth lets in
the system user of the same name, which `DynamicUser=` gives each unit. That way they don't have passwords.

For a remote PostgreSQL, set `DATABASE_URL` in `/etc/vialo/api.env` instead.

The Debian package is built with the `migrate` feature, so the service applies
migrations on startup. Without it, run them yourself before the first start:

```sh
cargo sqlx migrate run
```

`vialo dump`, `restore` and `seed` need a role too. Locally, run them as the
`postgres` superuser from a directory it can write to:

```sh
cd /var/lib/postgresql && sudo -u postgres vialo dump --database-url 'postgresql:///vialo?host=/run/postgresql'
```

## systemd

The Debian package (`cargo-deb`) installs `vialo-api` and the `vialo` CLI
(db init/dump/restore/seed; `pg_dump`/`pg_restore` come from the recommended
`postgresql-client`). On first install it creates `/etc/vialo/api.env` with a
generated `ENCRYPTION_KEY`

> [!WARNING]
> Back up `/etc/vialo/api.env`, as it contains the encryption key
> for WiFi passwords etc.

 Copy `vialo.example.toml` from  `/usr/share/doc/vialo-api/` to `/etc/vialo/vialo.toml`.

It ships three units, all enabled on install:

### Service unit (`vialo-api.service`)

- `Type=exec`, runs under `DynamicUser=yes` with no capabilities
- Sandboxed: `ProtectSystem=strict`, `PrivateDevices=yes`, `NoNewPrivileges=yes`
- Only `AF_UNIX AF_INET AF_INET6` address families allowed
- Reads `/etc/vialo/api.env` for environment variables
- `ConfigurationDirectory=vialo`, `LogsDirectory=vialo-api` and
  `StateDirectory=vialo-api` are the only writable paths
- Runs in `/var/lib/vialo-api`, so the default relative `media.dir` ends up
  in `/var/lib/vialo-api/media`

### Socket units (`vialo-api-public.socket`, `vialo-api-hooks.socket`)

systemd creates the listening sockets and passes them to the service, which
detects `LISTEN_FDS` and uses them instead of binding its own. Each socket has
its own directory, so you can expose one without the other:

| Socket | Address | Access |
|---|---|---|
| API public | `/run/vialo/public/api/api.sock` | group `vialo-public` |
| API hooks | `127.0.0.1:8011` | `HOOKS_KEY` (see [Hooks API](#hooks-api)) |

Both API sockets need to be active, otherwise the service falls back to
binding the addresses from `vialo.toml`. To skip socket activation, disable
both units.

The Unix sockets are mode 0660 and the group is created by the packages via
sysusers.d. Add the proxy user to it:

```sh
usermod -aG vialo-public caddy
```

For a reverse proxy in a container, bind-mount the directory rather than the
socket files, which get replaced whenever a unit restarts. Containers can't
resolve host group names, so pass the numeric gid
(`getent group vialo-public`):

```yaml
caddy:
  user: "1000:1000" # any non-root uid
  group_add: ["${VIALO_PUBLIC_GID}"]
  volumes:
    - /run/vialo/public:/run/vialo/public:ro
```

```
reverse_proxy unix//run/vialo/public/api/api.sock
```

A container running as root bypasses the group check entirely.

### Listen addresses

Configure `[public].listen` and `[hooks].listen` in `vialo.toml`:

| Use case | Config |
|---|---|
| Direct TCP | `listen = "127.0.0.1:8000"` |
| Direct Unix socket | `listen = "/run/vialo-api/public.sock"` |
| Socket activation | The `.socket` units provide the sockets; `vialo.toml` is ignored for binding |

### Hooks API
> [!CAUTION]
> Never expose the hooks API.

Kratos and FreeRADIUS call the hooks API, which hands out Wi-Fi passwords
(`/radius/*`) and writes identity data (`/hooks/update_identity`). Every
request needs `Authorization: Bearer <HOOKS_KEY>`.
Release builds refuse to start without a key.

The Debian package generates the key into `/etc/vialo/hooks.env`
on first install. Set the same key in:

- Kratos: the webhooks' `auth` config
- FreeRADIUS: `hooks_key` in the `vialo-rest` module

Kratos' webhooks can't use Unix sockets, which is why the listener is TCP.


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

Kratos authenticates with `HOOKS_KEY` (see [Hooks API](#hooks-api)). If both
vialo and Kratos are on the same host, use a Unix socket for the Kratos admin
API:

```
# .env
ORY_KRATOS_ADMIN_URL=unix:///run/kratos/admin.sock
```

## Printer subsystem

Requires the `printer` feature (enabled by default). The Konica Minolta backend isn't part of this repo, so it has no printer to talk to.

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

## NMS subsystem

Requires the `nms` feature (enabled by default).

The NMS subsystem pushes per‑account Wi‑Fi credentials to a UniFi or MikroTik controller.
It reads credentials from the `net_cred` table and the controller
hostname from `net_nms_connectors`.

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

The systemd unit at `vialo-dhcp/debian/service` applies the same
sandboxing as vialo-api (`DynamicUser=yes`, `ProtectSystem=strict`, etc.).

### Config

The server reads the `[dhcp]` section of the same `vialo.toml` as vialo-api.
`siaddr` must be set to an IP address on the listen interface.

Copy `vialo-dhcp/debian/dhcp.env.example` to `/etc/vialo/dhcp.env` for
`DATABASE_URL`; the unit points the binary at the config file itself. It runs
under `DynamicUser`, so
`/etc/vialo/vialo.toml` must be readable by an unknown UID — mode 0644,
with secrets left in the env file (0600, read by PID 1).

The DHCP server shares the same Postgres database as vialo-api.

### Container

`compose.dhcp.yaml` builds `vialo-dhcp/Dockerfile` (a scratch image, ~3 MB)
and runs it alongside the rest of the stack:

```sh
docker compose -f compose.yaml -f compose.dhcp.yaml up -d
```

It is a separate overlay rather than part of the base stack because bringing it
up binds UDP/67 and starts answering DHCP on whatever LAN the host is plugged
into.

The container needs `network_mode: host`, because clients broadcast from `0.0.0.0` and
the VLAN lookup reads netlink. That
in turn means Postgres has to be reachable on the host, so the overlay
publishes it on `127.0.0.1:5432` and points `DATABASE_URL` there.

Prefer the Debian package where you can: the systemd unit sandboxes the process
far more tightly than the container does, which only drops capabilities down to
`NET_BIND_SERVICE` and `NET_RAW`.

### VLAN scoping

The server reads MAC addresses and VLAN hints to look up devices in the
`net_device_info` view. VLANs come from one of two sources, in priority order:

1. **Option 82 Agent Circuit ID**: parsed from the DHCP relay agent, format
   configured via `circuit_id_vlan` (`ascii` for Cisco-style, `binary` for a
   2-byte tag, or `off` to ignore).
2. **Interface**: for packets arriving on a VLAN subinterface, the kernel
   netlink interface provides the VLAN ID.
