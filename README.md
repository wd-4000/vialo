# vialo

vialo is a platform for student dorms. Residents manage their own devices, bookings, and
network credentials. Everything significant is logged. Operational settings like network
topology, booking schedules, pricing, and groups are configurable at runtime. The backend
(`vialo-api`) is designed to be reusable(-ish) across deployments.

> [!CAUTION]
> Under heavy development. Not ready for production.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the **LLM policy**, as well as setup instructions, code conventions,
and how to run the app locally.

## Docs

| Doc | What it covers |
|---|---|
| [Concepts](docs/concepts.md) | Domain model : accounts, groups, realms, bookables, credits |
| [Configuration](docs/configuration.md) | `vialo.toml` reference and environment variables |
| [Deployment](docs/deployment.md) | Production setup: systemd, Postgres, reverse proxy, subsystems |
| [Architecture](docs/architecture.md) | Codebase structure, subsystem model, auth flow, database patterns |
| [API env vars](vialo/vialo-api/debian/api.env.example) | Quick reference for all environment variables |

## Stack

| Layer | Technology |
|---|---|
| Backend | Rust (axum, sqlx, tokio) |
| Database | PostgreSQL 18 |
| Frontend | Nuxt (not in this repo) |
| Identity | Ory Kratos (not in this repo) |


## Repository structure

```
vialo/
├── compose.yaml      # Docker
├── vialo-api/        # Rust backend (generic)
└── postgres/         # Custom PostgreSQL image
```

## API

vialo-api exposes two HTTP listeners (ports configured in `vialo.toml`):

| Listener | Purpose |
|---|---|
| `[public]` | Browser-facing REST API with CORS, auth, and rate limiting |
| `[hooks]` | Unauthenticated webhooks for Kratos identity sync and RADIUS auth |
