use sqlx::postgres::{PgPool, PgPoolOptions};

/// A small, fixed pool: one server process on a small machine, possibly
/// sharing its PostgreSQL server with other applications, so stay well under
/// a typical `max_connections`.
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
}

/// Migrations are embedded in the binary and run automatically at startup
/// rather than via a separate `sqlx migrate run` step, so `docker compose
/// up` (and every deploy) always runs the exact schema the running binary
/// expects with no extra tooling required on the VPS.
///
/// Air is intentionally a single-replica service, so startup owns migration
/// serialization and a failed migration keeps the container unhealthy. This
/// avoids shipping a second migration binary while preserving fail-fast
/// deploys.
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!().run(pool).await
}
