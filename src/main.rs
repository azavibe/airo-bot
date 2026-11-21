use std::net::SocketAddr;
use std::sync::Arc;

use tracing_subscriber::EnvFilter;

use airobot::config::Config;
use airobot::state::AppState;
use airobot::{db, email, push};

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env();
    if std::env::args().any(|arg| arg == "--check-smtp") {
        match email::Mailer::new(config.smtp.as_ref())
            .check_connection()
            .await
        {
            Ok(()) => tracing::info!("SMTP TLS and authentication check passed; no email sent"),
            Err(error) => {
                tracing::error!(%error, "SMTP preflight failed");
                std::process::exit(1);
            }
        }
        return;
    }

    // Fail fast: if Air can't reach its database, it should not come up
    // looking healthy.
    let pool = db::connect(&config.database_url)
        .await
        .expect("failed to connect to database");

    db::run_migrations(&pool)
        .await
        .expect("failed to run migrations");

    match airobot::crypto::encrypt_legacy_provider_keys(&pool).await {
        Ok(0) => {}
        Ok(n) => tracing::info!(count = n, "encrypted legacy plaintext provider keys"),
        Err(error) => tracing::error!(%error, "failed to encrypt legacy provider keys"),
    }

    let mailer = email::Mailer::new(config.smtp.as_ref());
    let push = push::PushSender::new(config.vapid.as_ref()).expect("failed to configure Web Push");
    let port = config.port;

    let (note_sender, _) = tokio::sync::broadcast::channel(256);
    let state = AppState {
        pool,
        config: Arc::new(config),
        mailer: Arc::new(mailer),
        push: Arc::new(push),
        note_sender,
        record_sender: tokio::sync::broadcast::channel(256).0,
        dispatcher_wake: Arc::new(tokio::sync::Notify::new()),
        local_ai: None,
    };

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(%addr, "starting airobot");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");
    airobot::local::serve(state, listener, shutdown_signal())
        .await
        .expect("server error");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl-C handler")
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
    tracing::info!("shutdown requested; draining in-flight requests");
}
