# Rust builds are CPU-heavy and slow on a small server, so dependency
# compilation is cached separately from application code. cargo-chef
# splits the build into a "recipe" (Cargo.toml/Cargo.lock only) and the real
# build: the deps layer only invalidates when dependencies actually change,
# not on every source edit. A cold build still takes several minutes.
# The React SPA is built here from web/src, so the image never ships a
# stale committed web/dist.
FROM node:22-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json web/.npmrc ./
RUN npm ci --no-audit --no-fund
COPY web ./
RUN npm run build

FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef
WORKDIR /app

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Deps layer -- cached as long as recipe.json (i.e. Cargo.lock) is unchanged.
RUN cargo chef cook --release --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# Compiled in, not read at runtime: askama renders templates at build time,
# and `sqlx::migrate!()` embeds migrations/*.sql into the binary so the
# running container carries its own schema with no separate migration step.
COPY templates ./templates
COPY migrations ./migrations
COPY assets ./assets
RUN cargo build --release --bin airobot

# Slim runtime: no Rust toolchain, no build cache, just the binary. Reuse the
# builder's CA bundle instead of running apt during every deploy; that keeps
# outbound SMTP/Web Push TLS working without making builds depend on a Debian
# package mirror. A numeric, unprivileged UID does not need an /etc/passwd row.
FROM debian:bookworm-slim AS runtime
COPY --from=chef /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /app/target/release/airobot /usr/local/bin/airobot
# The server serves the SPA from ./web/dist at runtime (tower-http ServeDir),
# so it must exist next to the working directory.
WORKDIR /app
COPY --from=web /web/dist ./web/dist
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/airobot"]
