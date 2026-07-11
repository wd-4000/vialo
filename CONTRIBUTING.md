# Contributing
## Note on LLM use
Large language models can be useful for reading into a codebase, or even writing new code.
However, let's at least not completely slop it up here.

I'm not going to pretend to have some moral high ground –¹ I've used LLMs for working on many parts of this project. Ideally this software would have been completely boutique, but I wanted to make it the best I could in the already glacial timeframe. I also acknowledge that they are problematic.

**If you see something I committed that's giving *vibe coded,*** I repent for my sins. Please flag it to me and I'll try to make it less annoying.


¹ human-placed em-dash
### If you must use an LLM: 
- Ask it to help you **review approaches** and pick one
- **Watch** what it's doing
- **Read** your contributions before submitting
- Make sure your contribution is not **annoying**. I will clarify this if needed.


## Setup

You need:

- [Rust](https://rust-lang.org) (latest stable)
- Node.js with Corepack enabled (`corepack enable`)
- PostgreSQL 18+

### 1. Database

Set `DATABASE_URL` in a `.env` file at the repo root:

```sh
DATABASE_URL=postgresql://user:pass@localhost:5432/vialo?schema=public
```

### 2. Migrations

```sh
cd vialo/vialo-api
cargo sqlx migrate run
```

### 3. API

Create a `vialo.toml` in the repo root. Use mock auth to skip Kratos:

```toml
[public]
cors_origins = ["http://localhost:3000", "http://localhost:3001"]
listen = "0.0.0.0:8000"

[hooks]
listen = "127.0.0.1:8011"

[auth]
type = "mock"
uuid = "1d63a916-36f9-4a14-9f9f-730b1fd58c12"
email = "test@example.com"

[org]
name = "Test Org"
domain = "test.example.com"
short_name = "TEST"
impressum = "https://example.com"
```

Then:

```sh
cargo run
```

The first start auto‑creates the mock admin account.

### 4. Frontends

```sh
cd vialo
yarn install
cd admin-ui && yarn run dev &
cd user-ui && yarn run dev &
```

## Code conventions

### Rust
- Use `sqlx::query!()` and `sqlx::query_scalar!()` macros. Never use runtime
  `sqlx::query()`. Queries are verified against the database at compile time.
- Preference for `const fn` and `let` bindings over mutable state where
  reasonable.

### TypeScript / Vue
- Use arrow function notation: `const f = () => {}`, not `function f() {}`.

### General
- Documentation updates in the same PR as the code change.
- Run `cargo check` before pushing. The CI will catch it anyway.

## Where to find things

| What | Where |
|---|---|
| What the words mean | [`docs/concepts.md`](docs/concepts.md) |
| How the code is organized | [`docs/architecture.md`](docs/architecture.md) |
| Config reference | [`docs/configuration.md`](docs/configuration.md) |
| Production deployment | [`docs/deployment.md`](docs/deployment.md) |
| Env vars quick reference | [`vialo/vialo-api/debian/api.env.example`](vialo/vialo-api/debian/api.env.example) |
| OpenAPI / REST reference | [https://wd-4000.github.io/vialo/](https://wd-4000.github.io/vialo/) |

## Resetting the dev database
- Stop the database container. (`docker compose stop postgres`)
- Delete the `data/postgres/18` directory. (`rm -rf data/postgres/18`)
- Restart the database container. (`docker compose up -d postgres`)
