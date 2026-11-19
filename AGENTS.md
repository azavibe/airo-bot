# AGENTS.md

Guidance for AI coding agents (and people) changing this repository. Read
[CONTRIBUTING.md](CONTRIBUTING.md) for the pull request process.

## Project

Airobot is a virtual office for small businesses, staffed by AI bots. Offices
have a group chat, bots, apps (Kanban boards and tables), tasks, approvals and
connections to outside services. External agents join through a device-code
sign-in and coordinate through threads, notes and tasks. Follow
`docs/ui-spec.md` and `docs/ui-language.md` for interface work.

## Stack and layout

- Rust 2021 with Axum and Tokio
- PostgreSQL via SQLx; migrations are embedded in the binary
- The browser UI is the React + TypeScript + Tailwind SPA in `web/`, built
  into `web/dist` and served by Axum. Askama templates remain the
  server-rendered fallback and the HTML side of content negotiation. Do not
  add a second frontend framework.
- `src/main.rs`: process startup, configuration, database, and HTTP server
- `src/lib.rs`: router and application modules
- `src/handlers/`: HTTP handlers grouped by feature
- `src/office/`: office setup, apps, budget gate, task dispatcher
  (server-executed tasks), procedure evolver, and the office group chat
  (`group.rs`: the Architect routes messages to bots)
- `src/connectors.rs`: connections to Google, Stripe, Shopify and webhooks;
  handlers in `src/handlers/connections.rs`
- `src/crypto.rs`: encryption at rest for third-party provider keys and
  derived webhook tokens
- `src/local.rs`: local mode used by the desktop app (loopback only, one
  owner, the user's own AI tools, no credits)
- `src/auth/`: secret generation and verification
- `templates/`: Askama HTML templates
- `web/`: React SPA sources (`npm ci && npm run build` in `web/`)
- `src-tauri/`: the desktop app. Keep its Rust surface small; product
  features belong in the shared server and web app.
- `migrations/`: append-only SQL migrations
- `tests/`: integration tests against a real PostgreSQL database

Do not substitute the stack. Do not add a cache, message queue, search engine,
second database, or another frontend. Prefer a database index when a query is
slow. Do not add `tower` or `hyper` as application dependencies; Axum provides
them (`tower` is a test-only dependency).

## Checks

Integration tests need a reachable PostgreSQL through `DATABASE_URL`
(`docker compose up postgres -d` starts one). Before handing work off, run:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
(cd web && npm run build)   # when web/ changed
```

`scripts/verify.sh` (or `scripts/verify.ps1`) runs all of them plus the Docker
image build. Say which checks you could not run and why.

## Implementation rules

- Create a new migration for every schema change. Never edit an existing
  migration: SQLx checksums them and deployed databases would refuse to start.
- Every table has `id uuid primary key default gen_random_uuid()`,
  `created_at timestamptz not null default now()`, and
  `updated_at timestamptz not null default now()`.
- Soft-delete user content with `deleted_at timestamptz`; never hard-delete it.
- Store secrets only as Argon2 hashes. Never persist raw keys or tokens. The
  one exception is a third-party credential the server must present again
  (a user's own AI provider key): store it only through `crypto::encrypt`.
- Authorize every object by its workspace or owner before reading or writing
  it, including ids that arrive in request bodies.
- Use the shared visibility values `private`, `unlisted`, and `public`, with
  `private` as the default.
- Every write endpoint accepts an optional `idempotency_key`.
- Negotiate responses on `Accept`: return JSON to JSON clients and HTML to
  browsers from the same route and handler.
- Log internal error details, but return only a short safe public message.
- Do not use `unwrap()` in request handlers.
- Give every endpoint an integration test that exercises the real router and
  a real test database.
- If the design does not decide something, choose the simpler option and
  leave a `TODO(air):` comment that names the tradeoff.
- Design for one small server handling about a million requests a month;
  avoid needless infrastructure and abstraction.

## Versions and changelog

The server (`Cargo.toml`), the desktop app (`src-tauri/Cargo.toml`,
`src-tauri/tauri.conf.json`) and the web app (`web/package.json`) share one
semantic version. Put user-visible changes under "Unreleased" in
`CHANGELOG.md`. Maintainers release by pushing a `v<version>` tag.
