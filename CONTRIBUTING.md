# Contributing
## LLMs
Large language models can be useful for reading into a codebase, or even writing new code.
However, let's at least not completely slop it up here.

I don't have a moral high ground —¹ I've used LLMs for working on many parts of this project. Ideally this software would have been completely boutique, but I wanted to make it the best I could in the already glacial time frame. I also acknowledge that they are problematic in many ways.

¹ human-placed em-dash

### Working with an LLM
- Ask it to help you review approaches and pick one
- Keep an eye on what it's doing
- Look through your contributions before submitting
- Try to avoid giant commits
- Avoid LLM-written copy if possible


## Local development setup
### 1. API
You need:

- [Rust](https://rust-lang.org) (latest stable)
- PostgreSQL 18+

Copy `vialo.example.toml` to `vialo.toml`.
Copy `.env.example` to `.env`.

Start the mock email server:
```sh
docker compose up -d mailhog
```

Start the database:
```sh
docker compose up -d postgres
```

Run the migrations:
```sh
cd vialo-api
cargo sqlx migrate run
```

Start the API
```sh
cargo run
```

The first start auto‑creates the mock admin account.
The API will run on [localhost:8000](http://localhost:8000) by default.


## Conventions
### Commits
Prefix your commits like so: `[crate]: [area]: summary`. For example, `vialo-api: posts: fix cover`.

When a commit touches several crates, list them in this order: vialo-common, vialo-api, vialo-dhcp.

### Rust
- Try to move structs used in both HTTP and subsystem parts of the app into the subsystems.
- Use `sqlx::query!()` and `sqlx::query_scalar!()` macros. Never use runtime
  `sqlx::query()` so that queries are verified at compile time
- Avoid `SELECT *`/`RETURNING *` in queries. There are some annoyances caused by column order.

#### vialo-dhcp
- only builds on Linux. You can use [vialo-dhcp/Dockerfile.dev](vialo-dhcp/Dockerfile.dev) to spin up a development env.

#### After changes
- **SQL changed:** run `cargo sqlx prepare -- --all-features` and check `.sqlx`
  into Git.
  Also fine to run after multiple commits, if you're firing them off in quick succession,
  or just when merging back to main.
  Do it as a separate commit if it's a lot. The container builds are
  offline (`SQLX_OFFLINE=true`) and enable non-default features, so a cache
  prepared without them is missing queries.
- **API surface changed:** nothing to commit. CI generates the OpenAPI spec and publishes it to the reference below.
  To look at it locally: `cargo run --bin openapi-spec -- openapi.json openapi-hooks.json`.

## Where to find things

| What | Where |
|---|---|
| What the words mean | [`docs/concepts.md`](docs/concepts.md) |
| How the code is organized | [`docs/architecture.md`](docs/architecture.md) |
| Config reference | [`docs/configuration.md`](docs/configuration.md) |
| Production deployment | [`docs/deployment.md`](docs/deployment.md) |
| Env vars quick reference | [`.env.example`](.env.example) |
| OpenAPI / REST reference | [https://wd-4000.github.io/vialo/](https://wd-4000.github.io/vialo/) |

## Resetting the dev database
- Stop the database container. (`docker compose stop postgres`)
- Delete the `data/postgres/18` directory. (`rm -rf data/postgres/18`)
- Restart the database container. (`docker compose up -d postgres`)
- Re-run the migrations. (`cargo sqlx migrate run`)
