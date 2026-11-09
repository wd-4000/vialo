# vialo

vialo is a student dorm platform meant to package up a lot of things you need into a nice little box.

Operational settings like network topology, booking schedules, pricing, and groups are configurable at runtime.
Residents can manage their own devices, bookings, and network credentials.
Everything significant is logged.

The backend bits (`vialo-api`, `vialo-dhcp`) are designed to be more or less reusable across deployments.

## Demo
Pre-requisite: Docker
```
cp vialo.example.toml vialo.toml
cp .env.example .env
docker compose -f compose.yaml -f compose.demo.yaml up
```
This pulls a prebuilt `vialo-api` image from ghcr instead of compiling the Rust
workspace. Plain `docker compose up` also works, that builds everything from
source with dev settings (see `compose.override.yaml`).

## Development
See [CONTRIBUTING.md](CONTRIBUTING.md) for the **LLM policy**, setup, code conventions, etc.

## Configuration
Config is in env variables and `vialo.toml`, see [docs/configuration.md](docs/configuration.md). 

Quick env reference: [.env.example](.env.example) 

## Deployment
### systemd
[docs/deployment.md](docs/deployment.md)

## Concepts and architecture
- [docs/concepts.md](docs/concepts.md) goes over the models: accounts, groups, realms, bookables, credits etc.
- [docs/architecture.md](docs/architecture.md) talks about the more abstract parts such as codebase structure, subsystem model, auth flow, and database patterns

## Repository structure

```text
├── compose.yaml
├── docs/               # Docs
├── vialo-api/          # Rust backend (generic)
├── vialo-common/       # Config loader shared by vialo-api & vialo-dhcp
├── vialo-dhcp/         # DHCP server based on dora [1]
└── postgres/           # Postgres image for vialo
```
1. DHCP: [dora](https://github.com/bluecatengineering/dora)

Frontends and auth ([Ory Kratos](https://github.com/ory/kratos)) are not in this repo.

## API

vialo-api exposes two HTTP listeners (ports configured in `vialo.toml`):

| Listener | Purpose |
|---|---|
| `[public]` | Browser-facing REST API with CORS, auth, and rate limiting |
| `[hooks]` | Unauthenticated webhooks for Kratos identity sync and RADIUS auth |

OpenAPI reference: [https://wd-4000.github.io/vialo/](https://wd-4000.github.io/vialo/)
