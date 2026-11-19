# Contributing to Airobot

Thanks for helping. Bug reports, fixes, docs and features are all welcome.

## Before you start

- **Bugs:** open an issue with the steps to reproduce it. Search first in case
  it is already reported.
- **Features and larger changes:** open an issue or a discussion before writing
  code, so we can agree on the approach. Small fixes can go straight to a pull
  request.
- **Security problems:** do not open a public issue; see [SECURITY.md](SECURITY.md).
- Looking for something to do? Try issues labelled `good first issue` or
  `help wanted`.

## Set up

You need Rust (stable), Node.js 22, and Docker (for PostgreSQL).

```sh
git clone https://github.com/<you>/airobot.git
cd airobot
cp .env.example .env
docker compose up postgres -d
(cd web && npm ci && npm run build)
cargo run
```

The app runs at <http://127.0.0.1:8081> with the example `.env`. Sign-in links
are printed in the server log; `AIR_DEV_LOGIN=1` adds a one-click sign-in for
`@example.com` addresses.

For the web app with hot reload, run `npm run dev` in `web/` alongside
`cargo run`.

## Make a change

1. Fork the repository and create a branch from `main`.
2. Keep the pull request to one topic. Read [AGENTS.md](AGENTS.md): it lists
   the rules the code follows (migrations, authorization, tests, no
   `unwrap()` in handlers). They apply to people as much as to AI tools.
3. Add or update tests. Every endpoint has an integration test that runs the
   real router against a real PostgreSQL.
4. Add a line under "Unreleased" in [CHANGELOG.md](CHANGELOG.md) if users
   will notice the change.
5. Run the checks:

   ```sh
   cargo fmt --check
   cargo clippy --all-targets -- -D warnings
   DATABASE_URL=postgres://airobot_user:changeme@localhost:5432/airobot cargo test
   (cd web && npm run build)
   ```

   `scripts/verify.sh` runs them all, plus the Docker build.
6. Open a pull request and fill in the template. CI runs the same checks.

AI-assisted contributions are fine. You are responsible for every line: read
it, run it and be ready to explain it.

## Contributor License Agreement

The first time you open a pull request, a bot asks you to agree to the
[Contributor License Agreement](CLA.md) by posting a comment. You keep the
copyright to your work; the CLA lets the project keep offering Airobot under
the AGPL and under other licenses (for example to fund Airobot Cloud). You
only sign once.

## Code of conduct

Everyone taking part is expected to follow the
[Code of Conduct](CODE_OF_CONDUCT.md).
