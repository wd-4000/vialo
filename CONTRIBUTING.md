# Contributing
## Development setup
### Dependencies
You need:
- [Rust](https://rust-lang.org) 1.100 via rustup (pinned in `rust-toolchain.toml`)
- PostgreSQL 18+ (use the container)

Here's a detailed tutorial:

#### Windows
If you're on Windows, install WSL using PowerShell (run as admin):
`wsl --install`. If asked, reboot, and run `wsl --install` again.
Create your user.

After you are done, follow the Ubuntu instructions below.

#### Ubuntu
0. Download the repository
    - Create an SSH key if you don't have one: `ssh-keygen -t ed25519 -C <your github email here!>`. Add it to GitHub using the [settings](https://github.com/settings/keys)
    - Clone the repo: `git clone git@github.com:wd-4000/vialo.git`
    - Enter the folder: `cd vialo`.
1. `sudo apt install build-essential rustup pkg-config libssl-dev`
2. Install Docker
    - You can use [Docker Desktop](https://docs.docker.com/desktop/)
     
      > [!NOTE]
      > If you're on WSL, download Docker Desktop for Windows and enable the default WSL integration in settings.
    
    - Or just grab the [Engine](https://docs.docker.com/engine/install/ubuntu/).
3. Install SQLx: `cargo install sqlx-cli`

### Copy the Configuration
```sh
cp vialo.example.toml vialo.toml
cp .env.example .env
```

### Launch
Start the mock email server (optional):
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
Some warnings will flash by if you didn't start the email server, this is okay. Keep it running.


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
only builds on Linux. You can use its [Dockerfile.dev](vialo-dhcp/Dockerfile.dev) to spin up a development environment if you're not using linux.

#### Before opening a PR
- Format the files: `cargo fmt --all` (your editor should do this automatically)
- Clippy can tell you things about the code quality: `cargo clippy`
- **SQL changed:** run `cargo sqlx prepare -- --all-features` and check `.sqlx`
  into Git.
  Also fine to run after multiple commits, if you're firing them off in quick succession,
  or just when merging back to main.
  Do it as a separate commit if it's a lot. The container builds are
  offline (`SQLX_OFFLINE=true`) and enable non-default features, so a cache
  prepared without them is missing queries.
- **API surface changed:** nothing to commit. CI generates the OpenAPI spec and publishes it to the reference below.
  To look at it locally: `cargo run --bin openapi-spec -- openapi.json openapi-hooks.json`.

#### Upgrade dependencies
Make sure you have `cargo-upgrade` installed. If not, `cargo install cargo-upgrades`.

Then, from the repository root occasionally run `cargo upgrade -i`. 
> [!WARNING]
> If this causes the build to fail and you can't fix it, revert the changes and run `cargo upgrade` without the -i.

Check the build by running `cargo check` inside `vialo-api` and `vialo-dhcp` (Don't forget that the latter only builds on Linux/inside WSL!)

## Note on LLMs
Large language models can be useful for reading into a codebase, or even writing new code.
However, let's be thoughtful and at least not completely slop it up here.

### Working with an LLM
- Ask it to help you review approaches and pick one
- Keep an eye on what it's doing
- Look through your contributions before submitting
- Try to avoid giant commits
- Avoid LLM-written copy if possible

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
