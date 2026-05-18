# vialo

vialo is a platform for student dorms.

> [!CAUTION]
> Under heavy development. Not ready for production.

API Docs: [https://wd-4000.github.io/vialo/](https://wd-4000.github.io/vialo/)
## Why

vialo is built around a few core ideas. Residents should be able to manage their own devices, bookings, and network credentials without involving the network team for routine tasks. Everything significant is logged, which matters when shared resources and credits are involved. Operational settings — network topology, booking schedules, pricing, groups — are configurable at runtime rather than hardcoded. The broader goal is one coherent platform instead of the mess of Directus, PHP, cronjobs, a C app, a Go app, and manual processes that preceded it, all with a good-looking UI.

## Features

### Users and groups
- User registration and account management
- Working group (AG) management and actives tracking (Aktivenliste)

### Network
- Per-user PPSK (Private Pre-Shared Key) Wi-Fi credential management via the UniFi API
  - UniFi integration, potential MikroTik and other vendor integration down the line 
- Flexible configuration: VLAN, IPv4/IPv6 subnets, NAT
- Postgres function as an endpoint for freeradius to authenticate users and do DHCP against

### Bookables
- Configurable bookable objects with time-based schedules and pricing
- Smart outlet control via NetIO integration (for example, powering an asset on booking start)
- Credits

### Community
- Posts and events feed with per-group permissions management (meant to provide user-generated events)
- Home dashboard with customizable quicklinks

## Env
Variables used by the UI (not in this repo):
| Name                | Description                                          |
|---------------------|------------------------------------------------------|
| ORY_BASE_URL        | Ory Kratos API endpoint (no trailing slash)          |
| URL_IMPRESSUM       | Impressum page                                       |
| URL_PRIVACY_POLICY  | Privacy Policy page                                  |
| COPYRIGHT           | Shown as follows in the footer: © <COPYRIGHT> <YEAR> |
| API_HOST            | Backend API base URL (no trailing slash)              |
| API_PATH            | Backend API base path                                 |
| EMAIL_DOMAIN        | Domain that follows the @ in generated email addresses |
| APPNAME_USER        | Short user-facing application name                    |
| APPNAME_USER_LONG   | Full user-facing application name                     |
| APPNAME_ADMIN       | Admin application name                                |

Variables used by vialo-api:
| Name                | Description                                                                                       |
|---------------------|---------------------------------------------------------------------------------------------------|
| DATABASE_URL        | postgresql://{postgres_root_username}:{postgres_root_password}@localhost:5432/vialo?schema=public |
| EMAIL_URL           | smtp://{email username}:{email password}@{email host}:{email port}/                               |
| EMAIL_DOMAIN        | Domain that follows the @ (group_slug@domain)                                                                       |


## Development
Create a `private` folder with the following files:
- postgres_root_password
- postgres_root_username

### Launch the database:

```sh
docker compose up postgres
```

This will initialize the database, storing the data in `data`.

### Start the API
Create a .env file in `vialo/vialo-api`:
```sh
DATABASE_URL = postgresql://{postgres_root_username}:{postgres_root_password}@localhost:5432/vialo?schema=public
EMAIL_URL = smtp://{email username}:{email password}@{email host}:{email port}/
EMAIL_DOMAIN = {email domain}
```

Use mock authentication to avoid having to bring up Kratos. The app will automatically provision an admin user for you.
Relevant portion of `vialo.toml`:

```toml
[auth]
type = "mock"
uuid = "1d63a916-36f9-4a14-9f9f-730b1fd58c12"
email = "test@example.com"
```

Make sure you have [Rust](https://rust-lang.org) installed. Then, you can use ``cargo run`` to start the API.


## Stack

| Layer | Technology |
|---|---|
| Backend | Rust |
| Database | PostgreSQL |
| Frontend | Nuxt  (not in this repo) |
| Identity | Ory Kratos  (not in this repo) |
| Auth proxy | Ory Oathkeeper  (not in this repo) |

## Repository structure

```
vialo/
├── compose.yaml          # Top-level service orchestration
├── vialo-api/            # Rust backend
└── postgres/             # Custom PostgreSQL image for account provisioning
```
