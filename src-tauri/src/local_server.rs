//! Runs Airobot itself on this computer, so the desktop app works with no
//! website, account or credits: a private PostgreSQL (shipped inside the
//! app) plus the same Airobot server, bound to 127.0.0.1 on a random port.
//! Chats and office tasks are answered by the AI tool the person chose
//! (Claude Code, Codex, OpenCode, Gemini CLI or Ollama).
//!
//! Everything lives in the app's data folder: the database, its password
//! and the key that encrypts saved provider keys. Each launch makes a new
//! sign-in secret that only the app's window knows.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use airobot::local::{LocalAi, LocalMode};
use postgresql_embedded::{PostgreSQL, Settings, VersionReq};

/// The PostgreSQL release the app ships (see the release workflow).
pub const POSTGRES_VERSION: &str = "16.16.0";
const DATABASE: &str = "airobot";

/// Where things are, decided by the app (or a test).
pub struct LocalPaths {
    /// Unpacked PostgreSQL release (`bin/`, `lib/`, `share/`).
    pub postgres: PathBuf,
    /// Private data folder: database files and secrets.
    pub data: PathBuf,
    /// Built web app (`index.html`, `assets/`).
    pub web: PathBuf,
}

/// A running local Airobot.
pub struct LocalServer {
    /// e.g. `http://127.0.0.1:53123`
    pub url: String,
    /// The one-launch secret the window enters with.
    pub token: String,
    pub pool: sqlx::PgPool,
    postgres: tokio::sync::Mutex<Option<PostgreSQL>>,
    shutdown: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl LocalServer {
    /// The address the window opens: signs in the owner, then the app.
    pub fn entry_url(&self) -> String {
        format!("{}/local/enter?token={}", self.url, self.token)
    }

    /// Stops the web server and PostgreSQL (on quit).
    pub async fn stop(&self) {
        if let Some(tx) = self.shutdown.lock().await.take() {
            let _ = tx.send(());
        }
        self.pool.close().await;
        if let Some(pg) = self.postgres.lock().await.take() {
            if let Err(error) = pg.stop().await {
                log::warn!("[Local] PostgreSQL did not stop cleanly: {error}");
            }
        }
    }
}

/// Windows hands out "verbatim" paths (`\\?\C:\Users\...`) for app folders.
/// PostgreSQL's own tools cannot find their neighbours through that form
/// (`initdb` reports "program postgres is needed ... but was not found"), so
/// the app gives them the ordinary drive path. Anything else is unchanged.
/// Provider apps for the desktop: the environment's first (a developer's
/// own), else Airobot's Google "Desktop app" client baked in at build time
/// (`AIROBOT_GOOGLE_DESKTOP_CLIENT_ID` / `_SECRET`, from the release
/// workflow's secrets). Google allows a desktop client any loopback port,
/// which is what this app answers on, and does not treat its secret as
/// confidential. Without either, Google is set up in Settings instead.
fn desktop_connectors() -> airobot::connectors::ConnectorApps {
    let mut apps = airobot::connectors::ConnectorApps::from_env();
    if apps.google.is_none() {
        let baked = (
            option_env!("AIROBOT_GOOGLE_DESKTOP_CLIENT_ID").map(str::trim),
            option_env!("AIROBOT_GOOGLE_DESKTOP_CLIENT_SECRET").map(str::trim),
        );
        if let (Some(id), Some(secret)) = baked {
            if !id.is_empty() && !secret.is_empty() {
                apps.google = Some(airobot::connectors::OAuthApp {
                    client_id: id.to_string(),
                    client_secret: secret.to_string(),
                });
            }
        }
    }
    apps
}

pub fn plain_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path.to_path_buf(),
    }
}

/// A secret kept in the data folder, made once (readable by this user only).
fn stored_secret(dir: &Path, name: &str) -> Result<String, String> {
    let path = dir.join(name);
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim().to_string();
        if existing.len() >= 32 {
            return Ok(existing);
        }
    }
    let fresh = airobot::auth::secret::random_hex_token(32);
    std::fs::write(&path, &fresh).map_err(|e| format!("cannot save {name}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(fresh)
}

/// Starts PostgreSQL and Airobot. Takes a few seconds; the first launch
/// also creates the database.
pub async fn start(paths: &LocalPaths, local_ai: LocalAi) -> Result<LocalServer, String> {
    let paths = &LocalPaths {
        postgres: plain_path(&paths.postgres),
        data: plain_path(&paths.data),
        web: plain_path(&paths.web),
    };
    log::info!(
        "[Local] starting: database {} data {}",
        paths.postgres.display(),
        paths.data.display()
    );
    if !paths.postgres.join("bin").is_dir() {
        return Err(format!(
            "this copy of the app is missing its database ({} has no bin folder)",
            paths.postgres.display()
        ));
    }
    std::fs::create_dir_all(&paths.data)
        .map_err(|e| format!("cannot create the data folder: {e}"))?;

    // Saved provider keys are encrypted with this install's own key.
    let provider_secret = stored_secret(&paths.data, "provider-key-secret")?;
    // SAFETY-equivalent note: set before the server reads it; the crypto
    // module reads it once, lazily, after this point.
    std::env::set_var("PROVIDER_KEY_SECRET", provider_secret);

    let settings = Settings {
        version: VersionReq::parse(&format!("={POSTGRES_VERSION}")).map_err(|e| e.to_string())?,
        installation_dir: paths.postgres.clone(),
        trust_installation_dir: true,
        data_dir: paths.data.join("postgres"),
        password_file: paths.data.join(".pgpass"),
        // The database keeps the password it was created with; reuse it.
        password: stored_secret(&paths.data, "database-password")?,
        host: "127.0.0.1".to_string(),
        port: 0,
        temporary: false,
        timeout: Some(Duration::from_secs(60)),
        ..Settings::default()
    };

    let mut postgres = PostgreSQL::new(settings);
    // A crash can leave the old server running; stop it before starting.
    if paths.data.join("postgres").join("postmaster.pid").exists() {
        let _ = postgres.stop().await;
    }
    log::info!("[Local] preparing the database (first start creates it)");
    postgres
        .setup()
        .await
        .map_err(|e| format!("could not prepare the database: {e}"))?;
    log::info!("[Local] starting the database");
    postgres
        .start()
        .await
        .map_err(|e| format!("could not start the database: {e}"))?;
    if !postgres
        .database_exists(DATABASE)
        .await
        .map_err(|e| format!("could not check the database: {e}"))?
    {
        postgres
            .create_database(DATABASE)
            .await
            .map_err(|e| format!("could not create the database: {e}"))?;
    }
    let database_url = postgres.settings().url(DATABASE);

    let pool = airobot::db::connect(&database_url)
        .await
        .map_err(|e| format!("could not connect to the database: {e}"))?;
    log::info!("[Local] updating the database");
    airobot::db::run_migrations(&pool)
        .await
        .map_err(|e| format!("could not update the database: {e}"))?;

    // Loopback only: nothing on the network can reach it.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("could not open a local port: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let url = format!("http://127.0.0.1:{port}");
    let token = airobot::auth::secret::random_hex_token(32);

    let config = airobot::config::Config {
        database_url,
        port,
        app_base_url: url.clone(),
        smtp: None,
        vapid: None,
        production: false,
        dev_login: false,
        openrouter_api_key: None,
        upgrade_url: None,
        local: Some(LocalMode {
            token: token.clone(),
        }),
        web_dist: paths.web.clone(),
        // Airobot's own Google sign-in, built in by the release workflow,
        // so Gmail, Calendar and Sheets connect with one click.
        connectors: desktop_connectors(),
        // The person's own AI tools do the searching, so the desktop allows more.
        web: airobot::web::WebLimits::from_env(true),
    };
    let (note_sender, _) = tokio::sync::broadcast::channel(256);
    let state = airobot::state::AppState {
        pool: pool.clone(),
        config: Arc::new(config),
        mailer: Arc::new(airobot::email::Mailer::new(None)),
        push: Arc::new(
            airobot::push::PushSender::new(None).map_err(|e| format!("push setup failed: {e}"))?,
        ),
        note_sender,
        dispatcher_wake: Arc::new(tokio::sync::Notify::new()),
        local_ai: Some(local_ai),
    };

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    // Same runtime as the database pool (the caller's).
    tokio::spawn(async move {
        let shutdown = async {
            let _ = rx.await;
        };
        if let Err(error) = airobot::local::serve(state, listener, shutdown).await {
            log::error!("[Local] Airobot server stopped: {error}");
        }
    });
    log::info!("[Local] Airobot is running at {url}");

    Ok(LocalServer {
        url,
        token,
        pool,
        postgres: tokio::sync::Mutex::new(Some(postgres)),
        shutdown: tokio::sync::Mutex::new(Some(tx)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_made_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("airobot-secret-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = stored_secret(&dir, "s").unwrap();
        let b = stored_secret(&dir, "s").unwrap();
        assert_eq!(a, b);
        assert!(a.len() >= 32);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn windows_verbatim_paths_become_plain_drive_paths() {
        let plain = |p: &str| plain_path(Path::new(p)).to_string_lossy().into_owned();
        assert_eq!(
            plain(r"\\?\C:\Users\me\AppData\Local\Airobot\postgres"),
            r"C:\Users\me\AppData\Local\Airobot\postgres"
        );
        assert_eq!(plain(r"\\?\UNC\server\share\pg"), r"\\server\share\pg");
        // Ordinary paths are left alone.
        assert_eq!(plain(r"C:\Airobot\postgres"), r"C:\Airobot\postgres");
        assert_eq!(plain("/opt/airobot/postgres"), "/opt/airobot/postgres");
    }

    /// The whole local app on a real PostgreSQL: start, sign in, chat with
    /// the computer's AI, stop, start again with the same data. Runs where
    /// `AIROBOT_TEST_POSTGRES` points at an unpacked PostgreSQL release
    /// (the desktop CI does this on macOS and Windows); PostgreSQL refuses
    /// to run as root, so it is skipped otherwise.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_local_app_runs_and_keeps_its_data() {
        let Some(pg) = std::env::var_os("AIROBOT_TEST_POSTGRES") else {
            eprintln!("AIROBOT_TEST_POSTGRES not set; skipping");
            return;
        };
        let root = std::env::temp_dir().join(format!("airobot-local-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let web = root.join("web");
        std::fs::create_dir_all(&web).unwrap();
        std::fs::write(
            web.join("index.html"),
            "<!doctype html><title>Airobot</title>",
        )
        .unwrap();
        // `canonicalize` gives the Windows "verbatim" form (`\\?\C:\...`)
        // the installed app receives from Tauri, so this covers that case.
        let paths = LocalPaths {
            postgres: std::fs::canonicalize(pg).unwrap(),
            data: std::fs::canonicalize(&root).unwrap().join("data"),
            web: std::fs::canonicalize(web).unwrap(),
        };
        let ai: LocalAi = Arc::new(|req: airobot::local::LocalAiRequest| {
            Box::pin(async move {
                assert!(req.system.contains("Cactus"));
                Ok("Hello from this computer.".to_string())
            })
        });

        let server = start(&paths, Arc::clone(&ai)).await.expect("first start");
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        assert!(client
            .get(format!("{}/health", server.url))
            .send()
            .await
            .unwrap()
            .status()
            .is_success());

        // The window's entry signs in the owner.
        let entered = client.get(server.entry_url()).send().await.unwrap();
        assert!(entered.status().is_redirection());
        let cookie = entered
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|v| v.starts_with("air_session="))
            .expect("session cookie")
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let me: serde_json::Value = client
            .get(format!("{}/api/me", server.url))
            .header("Cookie", &cookie)
            .header("Accept", "application/json")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(me["local_mode"], true);
        assert_eq!(me["email"], airobot::local::LOCAL_OWNER_EMAIL);

        // A chat is answered by the computer's AI, as the character.
        let agent: serde_json::Value = client
            .post(format!("{}/agents", server.url))
            .header("Cookie", &cookie)
            .header("Accept", "application/json")
            .json(&serde_json::json!({ "name": "Cactus", "handle": "cactus", "instructions": "You are Cactus." }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let agent_id = agent["agent"]["id"].as_str().expect("agent").to_string();
        let thread: serde_json::Value = client
            .post(format!("{}/threads", server.url))
            .header("Cookie", &cookie)
            .header("Accept", "application/json")
            .json(&serde_json::json!({ "title": "Garden" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let thread_id = thread["id"].as_str().expect("thread").to_string();
        sqlx::query("INSERT INTO thread_agents (thread_id, agent_id) VALUES ($1::uuid, $2::uuid)")
            .bind(&thread_id)
            .bind(&agent_id)
            .execute(&server.pool)
            .await
            .unwrap();
        let said = client
            .post(format!("{}/threads/{thread_id}/notes", server.url))
            .header("Cookie", &cookie)
            .header("Accept", "application/json")
            .json(&serde_json::json!({ "body": "Hi Cactus", "kind": "command" }))
            .send()
            .await
            .unwrap();
        assert!(said.status().is_success());
        let mut answered = false;
        for _ in 0..50 {
            let n: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM notes WHERE thread_id = $1::uuid AND body = 'Hello from this computer.'",
            )
            .bind(&thread_id)
            .fetch_one(&server.pool)
            .await
            .unwrap();
            if n > 0 {
                answered = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(answered, "the local AI answered the chat");
        server.stop().await;

        // Next launch: same data, same owner, new secret.
        let again = start(&paths, ai).await.expect("second start");
        let owners: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email = $1")
            .bind(airobot::local::LOCAL_OWNER_EMAIL)
            .fetch_one(&again.pool)
            .await
            .unwrap();
        assert_eq!(owners, 1);
        let notes: i64 = sqlx::query_scalar("SELECT count(*) FROM notes")
            .fetch_one(&again.pool)
            .await
            .unwrap();
        assert!(notes >= 2, "the conversation survived a restart");
        again.stop().await;
        let _ = std::fs::remove_dir_all(&root);
    }
}
