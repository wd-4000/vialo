# Contributing
## Note on LLM use
Large language models can be useful for reading into a codebase, or even writing new code.
However, let's at least not completely slop it up here.

I'm not going to pretend to have some moral high ground –¹ I've used LLMs for working on many parts of this project. Ideally this software would have been completely boutique, but I wanted to make it the best I could in the already glacial time frame. I also acknowledge that they are problematic in many ways.

**If you see something I committed that's giving *vibe coded,*** I repent for my sins. Please flag it to me and I'll try to make it less annoying.


¹ human-placed em-dash
### If you must use an LLM: 
- Ask it to help you **review approaches** and pick one
- **Watch** what it's doing
- **Read** your contributions before submitting
- Make sure your contribution is not **annoying**. I will clarify this if needed.


## Local development setup
### 1. API
You need:

- [Rust](https://rust-lang.org) (latest stable)
- Node.js with Corepack enabled (`corepack enable`)
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
cd vialo/vialo-api
cargo sqlx migrate run
```

Start the API
```sh
cargo run
```

The first start auto‑creates the mock admin account.
The API will run on [localhost:8000](http://localhost:8000) by default.


## Code conventions
### Rust
- Use `sqlx::query!()` and `sqlx::query_scalar!()` macros. Never use runtime
  `sqlx::query()` so that queries are verified at compile time
- Run `cargo sqlx prepare` after changes and check `.sqlx` into Git


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
