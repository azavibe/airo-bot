#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> Building the web app..."
[ -d web/node_modules ] || npm --prefix web ci --no-audit --no-fund
npm --prefix web run build

echo "==> Checking code format..."
cargo fmt --check

echo "==> Running clippy linter..."
cargo clippy --all-targets -- -D warnings

echo "==> Running test suite..."
cargo test

# If SMTP connection is configured in the environment, run auth-only test
if [ -n "${SMTP_HOST:-}" ]; then
    echo "==> Testing SMTP connection (auth-only)..."
    cargo test --lib configured_smtp_connection -- --ignored
else
    echo "==> SMTP_HOST not set; skipping live SMTP connection check."
fi

# Build local docker verification image if docker daemon is reachable
if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
    echo "==> Building docker image..."
    docker build -t air-verification:local .
fi

echo "All verification checks passed."
