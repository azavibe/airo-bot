pub mod engines;
pub mod local_server;

use engines::{Engine, EngineInfo};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// What the desktop bot remembers between launches, in the app's own data
/// folder. `agent_key` is this computer's Airobot bot key (from pairing),
/// not any AI provider's credential: those stay inside each tool.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct BotConfig {
    pub server_url: String,
    pub agent_key: String,
    pub agent_name: String,
    pub engine: Option<Engine>,
    pub enabled: bool,
    /// Answer the person's own chat messages here too (their subscription,
    /// on this computer). Takes effect while the bot is on.
    #[serde(default)]
    pub answer_chats: bool,
    /// Where the window works: this computer (the default, no account
    /// needed) or Airobot Cloud (sign in on the website).
    #[serde(default)]
    pub mode: AppMode,
}

#[derive(Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum AppMode {
    #[default]
    Local,
    Cloud,
}

/// The local Airobot once it has started (or why it couldn't).
#[derive(Default)]
pub struct DesktopState {
    pub server: tokio::sync::Mutex<Option<Arc<local_server::LocalServer>>>,
    pub start_error: Mutex<Option<String>>,
}

/// Shown while the local database starts, and with the reason if it can't.
const LOADING_PAGE: &str = "desktop-loading.html";

#[derive(Default)]
pub struct LocalBotState {
    pub enabled: AtomicBool,
    /// Bumped on every start; a loop exits as soon as it is no longer the
    /// current generation, so quick off/on toggles never leave two loops.
    pub generation: AtomicU64,
    pub config: Mutex<BotConfig>,
    pub config_path: Mutex<Option<PathBuf>>,
    /// The task the bot is working on right now, for the status panel.
    pub working_on: Mutex<Option<String>>,
    pub last_error: Mutex<Option<String>>,
}

impl LocalBotState {
    fn config(&self) -> BotConfig {
        self.config.lock().map(|c| c.clone()).unwrap_or_default()
    }

    fn save(&self, update: impl FnOnce(&mut BotConfig)) -> Result<BotConfig, String> {
        let mut guard = self
            .config
            .lock()
            .map_err(|_| "settings are busy".to_string())?;
        update(&mut guard);
        let snapshot = guard.clone();
        drop(guard);
        if let Some(path) = self.config_path.lock().ok().and_then(|p| p.clone()) {
            write_private(&path, &snapshot)?;
        }
        Ok(snapshot)
    }
}

/// Writes the settings readable by this user only: the file holds the
/// bot's Airobot key.
fn write_private(path: &std::path::Path, config: &BotConfig) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn load_config(path: &std::path::Path) -> BotConfig {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Only talk to the Airobot server over HTTPS, or plain HTTP on loopback.
/// The bot sends its credential to this URL, so an arbitrary URL from
/// the webview must never be accepted.
fn validate_server_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let url = if trimmed.is_empty() {
        "http://127.0.0.1:8081"
    } else {
        trimmed
    };
    let parsed = reqwest::Url::parse(url).map_err(|_| "invalid server URL".to_string())?;
    let loopback = matches!(
        parsed.host_str(),
        Some("127.0.0.1") | Some("localhost") | Some("[::1]")
    );
    match parsed.scheme() {
        "https" => Ok(url.to_string()),
        "http" if loopback => Ok(url.to_string()),
        _ => Err("server URL must use https (http only for localhost)".to_string()),
    }
}

/// The bot's Airobot key goes in `Authorization`.
fn with_key(req: reqwest::RequestBuilder, key: &str) -> reqwest::RequestBuilder {
    req.header("Authorization", format!("Bearer {key}"))
}

fn start_bot(state: &Arc<LocalBotState>, workdir: PathBuf) {
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    state.enabled.store(true, Ordering::SeqCst);
    let state = Arc::clone(state);
    // Tray and command callbacks don't necessarily run inside a Tokio
    // context; Tauri's runtime handle always works.
    tauri::async_runtime::spawn(async move {
        run_local_bot_loop(state, generation, workdir).await;
    });
}

fn stop_bot(state: &LocalBotState) {
    state.enabled.store(false, Ordering::SeqCst);
    state.generation.fetch_add(1, Ordering::SeqCst);
}

fn workdir(app: &AppHandle) -> PathBuf {
    app.path()
        .app_cache_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("airobot"))
        .join("bot-work")
}

#[derive(Serialize, Clone)]
pub struct LocalBotStatus {
    pub enabled: bool,
    pub paired: bool,
    pub agent_name: String,
    pub engine: Option<Engine>,
    pub answer_chats: bool,
    pub working_on: Option<String>,
    pub last_error: Option<String>,
    pub engines: Vec<EngineInfo>,
}

async fn status_of(state: &LocalBotState) -> LocalBotStatus {
    let config = state.config();
    LocalBotStatus {
        enabled: state.enabled.load(Ordering::SeqCst),
        paired: !config.agent_key.is_empty(),
        agent_name: config.agent_name,
        engine: config.engine,
        answer_chats: config.answer_chats,
        working_on: state.working_on.lock().ok().and_then(|w| w.clone()),
        last_error: state.last_error.lock().ok().and_then(|e| e.clone()),
        engines: engines::detect_all().await,
    }
}

/// Tells the server whether this computer answers its owner's chats. The
/// route is on only while the bot runs with "answer my chats" on, so chats
/// never wait for a computer that isn't working.
async fn set_chat_route(config: &BotConfig, on: bool) -> Result<(), String> {
    if config.agent_key.is_empty() {
        return Ok(());
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    with_key(
        client.post(format!("{}/agent/chat-route", config.server_url)),
        &config.agent_key,
    )
    .json(&serde_json::json!({ "enabled": on }))
    .send()
    .await
    .map_err(|_| "could not reach the Airobot server".to_string())?
    .error_for_status()
    .map_err(|_| "the server refused the chat setting".to_string())?;
    Ok(())
}

#[derive(Serialize, Clone)]
pub struct PairingCode {
    pub code: String,
    pub expires_in: i64,
}

#[derive(Deserialize)]
struct DeviceCode {
    code: String,
    device_code: String,
    expires_in: i64,
}

/// The Airobot web app the desktop window shows. The bundled copy of the
/// SPA cannot reach the server (its relative `/api` calls would go to the
/// webview's own origin), so the window opens the hosted app. Release
/// builds bake the URL in from `AIROBOT_APP_URL` at compile time (the
/// release workflow sets it); debug builds follow `APP_BASE_URL` at run
/// time so they can point at a local server.
pub fn app_url() -> String {
    const RELEASE_URL: &str = match option_env!("AIROBOT_APP_URL") {
        Some(url) => url,
        None => "https://airo-bot.com",
    };
    let raw = if cfg!(debug_assertions) {
        std::env::var("APP_BASE_URL").unwrap_or_else(|_| "http://127.0.0.1:8081".to_string())
    } else {
        RELEASE_URL.to_string()
    };
    validate_server_url(&raw).unwrap_or_else(|_| RELEASE_URL.to_string())
}

/// Creates or focuses the main desktop window.
/// Implements Herdr low-RAM architecture: when closed, the window was destroyed (0 MB GUI RAM).
/// Here we re-instantiate it cleanly on demand.
/// What the window should show now: the local app (once started), its
/// loading page, or the hosted site in cloud mode.
fn window_target(app: &AppHandle) -> WebviewUrl {
    let state = app.state::<Arc<LocalBotState>>();
    if state.config().mode == AppMode::Cloud {
        if let Ok(url) = app_url().parse() {
            return WebviewUrl::External(url);
        }
    }
    let desktop = app.state::<Arc<DesktopState>>();
    let ready = desktop.server.try_lock().ok().and_then(|s| s.clone());
    match ready.and_then(|server| server.entry_url().parse().ok()) {
        Some(url) => WebviewUrl::External(url),
        None => {
            let error = desktop.start_error.lock().ok().and_then(|e| e.clone());
            match error {
                Some(e) => {
                    WebviewUrl::App(format!("{LOADING_PAGE}?error={}", urlencode(&e)).into())
                }
                None => WebviewUrl::App(LOADING_PAGE.into()),
            }
        }
    }
}

fn urlencode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Points an open window at what it should show now.
fn refresh_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let target = match window_target(app) {
            WebviewUrl::External(url) => Some(url),
            WebviewUrl::App(path) => win
                .url()
                .ok()
                .and_then(|u| u.join(&format!("/{}", path.display())).ok()),
            _ => None,
        };
        if let Some(url) = target {
            let _ = win.navigate(url);
        }
    }
}

/// Creates or focuses the main desktop window.
pub fn create_or_focus_window(app: &AppHandle) -> Result<WebviewWindow, tauri::Error> {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(win);
    }

    let builder = WebviewWindowBuilder::new(app, "main", window_target(app))
        .title("Airobot")
        .inner_size(1280.0, 840.0)
        .min_inner_size(900.0, 600.0)
        .resizable(true)
        // Tauri's file-drop handler swallows the page's own drag and drop
        // (on Windows every drag), which broke moving Kanban cards. The app
        // takes no dropped files, so the page keeps drag and drop.
        .disable_drag_drop_handler();
    // No system title bar: the app draws its own (drag area and window
    // buttons in the page, see web/src/components/DesktopTitleBar.tsx),
    // so the window looks the same in every theme. macOS keeps its
    // traffic lights over the page.
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.decorations(false);

    let window = builder.build()?;
    attach_low_ram_close_handler(&window);
    let _ = window.set_focus();
    Ok(window)
}

/// The AI tool that answers a local chat or task: the bot's own choice
/// (with its model) when that tool is on this computer, else the tool chosen
/// for this computer. With none chosen yet, the first one that is installed
/// and signed in is used (and remembered).
fn local_ai_for(state: Arc<LocalBotState>, workdir: PathBuf) -> airobot::local::LocalAi {
    Arc::new(move |req: airobot::local::LocalAiRequest| {
        let state = Arc::clone(&state);
        let workdir = workdir.clone();
        Box::pin(async move {
            let prompt = format!("{}\n\n{}", req.system, req.prompt);
            let bot_engine = req
                .engine
                .as_deref()
                .and_then(|e| serde_json::from_value::<Engine>(serde_json::json!(e)).ok());
            if let Some(engine) = bot_engine {
                if engines::installed(engine).await {
                    log::info!(
                        "[Local AI] the bot's own choice: {engine:?} {:?}",
                        req.model
                    );
                    return engines::run_with(
                        engine,
                        req.model.as_deref(),
                        &prompt,
                        &workdir,
                        req.web,
                    )
                    .await;
                }
                log::warn!("[Local AI] the bot's {engine:?} is not installed; using the default");
            }
            // A remembered tool that has since been uninstalled must not
            // fail every chat: look again for one that works.
            let remembered = match state.config().engine {
                Some(engine) if engines::installed(engine).await => Some(engine),
                _ => None,
            };
            let engine = match remembered {
                Some(engine) => engine,
                None => {
                    let found = engines::detect_all()
                        .await
                        .into_iter()
                        .find(|e| e.installed && e.signed_in != Some(false))
                        .map(|e| e.id)
                        .ok_or_else(|| {
                            "no AI tool found on this computer (install Claude Code, Codex, OpenCode, Gemini CLI or Ollama)"
                                .to_string()
                        })?;
                    let _ = state.save(|c| c.engine = Some(found));
                    found
                }
            };
            log::info!(
                "[Local AI] this computer's default: {engine:?} (web: {})",
                req.web
            );
            engines::run_with(engine, None, &prompt, &workdir, req.web).await
        })
    })
}

/// Where the app keeps its parts: the shipped PostgreSQL and web app (in
/// the app's resources) and the person's data.
fn local_paths(app: &AppHandle) -> Result<local_server::LocalPaths, String> {
    let resources = app.path().resource_dir().map_err(|e| e.to_string())?;
    let data = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("local");
    // Development builds can point at an unpacked PostgreSQL and the
    // repository's web build instead.
    let postgres = std::env::var_os("AIROBOT_POSTGRES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| resources.join("postgres"));
    let web = if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../web/dist")
    } else {
        resources.join("web")
    };
    Ok(local_server::LocalPaths {
        postgres,
        data,
        web,
    })
}

/// Starts the local Airobot in the background, then shows it.
fn start_local(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let bot = Arc::clone(&*app.state::<Arc<LocalBotState>>());
        let desktop = Arc::clone(&*app.state::<Arc<DesktopState>>());
        let result = match local_paths(&app) {
            Ok(paths) => {
                // Never spin forever: a start that takes this long is stuck.
                let limit = std::time::Duration::from_secs(180);
                match tokio::time::timeout(
                    limit,
                    local_server::start(&paths, local_ai_for(bot, workdir(&app))),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err("starting took longer than 3 minutes; see the log file in the app's log folder".to_string()),
                }
            }
            Err(e) => Err(e),
        };
        match result {
            Ok(server) => {
                *desktop.server.lock().await = Some(Arc::new(server));
            }
            Err(e) => {
                log::error!("[Local] could not start: {e}");
                if let Ok(mut slot) = desktop.start_error.lock() {
                    *slot = Some(e);
                }
            }
        }
        refresh_window(&app);
    });
}

/// Attaches the low-RAM close handler:
/// Window destruction completely terminates WebView2, dropping GUI RAM to 0 MB.
fn attach_low_ram_close_handler(window: &WebviewWindow) {
    let win = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            log::info!(
                "[Low-RAM] Main window close requested -> destroying WebView window to free memory"
            );
            let _ = win.destroy();
        }
    });
}

pub mod commands {
    use super::*;

    /// Detected AI tools and the bot's state, for the Connect panel.
    #[tauri::command]
    pub async fn local_bot_status(
        state: State<'_, Arc<LocalBotState>>,
    ) -> Result<LocalBotStatus, String> {
        Ok(status_of(&state).await)
    }

    /// Starts pairing this computer as a bot: asks the server for a device
    /// code and waits in the background for it to be approved (the web app
    /// approves it for the signed-in person), then keeps the bot key.
    #[tauri::command]
    pub async fn local_bot_pair(
        server_url: String,
        state: State<'_, Arc<LocalBotState>>,
    ) -> Result<PairingCode, String> {
        let server = validate_server_url(&server_url)?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;
        let issued: DeviceCode = client
            .post(format!("{server}/device/code"))
            .header("Accept", "application/json")
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|_| "could not reach the Airobot server".to_string())?
            .error_for_status()
            .map_err(|_| "the server refused to start pairing".to_string())?
            .json()
            .await
            .map_err(|_| "unexpected pairing response".to_string())?;

        let state = Arc::clone(&state);
        let device_code = issued.device_code.clone();
        let expires_in = issued.expires_in.clamp(30, 900);
        tauri::async_runtime::spawn(async move {
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(expires_in as u64);
            while std::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let Ok(resp) = client
                    .post(format!("{server}/device/token"))
                    .json(&serde_json::json!({ "device_code": device_code }))
                    .send()
                    .await
                else {
                    continue;
                };
                let Ok(body) = resp.json::<serde_json::Value>().await else {
                    continue;
                };
                match body.get("status").and_then(|s| s.as_str()) {
                    Some("approved") => {
                        let key = body
                            .get("agent_key")
                            .and_then(|k| k.as_str())
                            .unwrap_or_default();
                        let name = body
                            .get("agent_name")
                            .and_then(|k| k.as_str())
                            .unwrap_or("Desktop Bot");
                        if !key.is_empty() {
                            let _ = state.save(|c| {
                                c.server_url = server.clone();
                                c.agent_key = key.to_string();
                                c.agent_name = name.to_string();
                            });
                        }
                        return;
                    }
                    Some("pending") => continue,
                    _ => return,
                }
            }
        });
        Ok(PairingCode {
            code: issued.code,
            expires_in: issued.expires_in,
        })
    }

    /// Turns the bot on or off and chooses which AI tool does its work.
    #[tauri::command]
    pub async fn local_bot_configure(
        app: AppHandle,
        enabled: bool,
        engine: Option<Engine>,
        state: State<'_, Arc<LocalBotState>>,
    ) -> Result<LocalBotStatus, String> {
        let config = state.save(|c| {
            if engine.is_some() {
                c.engine = engine;
            }
            c.enabled = enabled;
        })?;
        stop_bot(&state);
        if enabled {
            if config.agent_key.is_empty() {
                return Err("pair this computer first".to_string());
            }
            if config.engine.is_none() {
                return Err("choose which AI the bot should use".to_string());
            }
            start_bot(&state, workdir(&app));
        }
        if config.answer_chats {
            set_chat_route(&config, enabled).await?;
        }
        Ok(status_of(&state).await)
    }

    /// Answer the owner's own chats on this computer (with the chosen AI
    /// tool) instead of the server's AI, while the bot is on.
    #[tauri::command]
    pub async fn local_bot_set_chat(
        enabled: bool,
        state: State<'_, Arc<LocalBotState>>,
    ) -> Result<LocalBotStatus, String> {
        let config = state.config();
        if enabled && config.agent_key.is_empty() {
            return Err("pair this computer first".to_string());
        }
        set_chat_route(&config, enabled && config.enabled).await?;
        state.save(|c| c.answer_chats = enabled)?;
        Ok(status_of(&state).await)
    }

    /// Which mode the window is in, for the web app's settings.
    #[tauri::command]
    pub async fn desktop_info(
        state: State<'_, Arc<LocalBotState>>,
        desktop: State<'_, Arc<DesktopState>>,
    ) -> Result<serde_json::Value, String> {
        let local_url = desktop.server.lock().await.as_ref().map(|s| s.url.clone());
        Ok(serde_json::json!({
            "mode": state.config().mode,
            "local_ready": local_url.is_some(),
            "cloud_url": app_url(),
            "start_error": desktop.start_error.lock().ok().and_then(|e| e.clone()),
        }))
    }

    /// Switches the window between this computer and Airobot Cloud.
    #[tauri::command]
    pub async fn desktop_set_mode(
        app: AppHandle,
        mode: AppMode,
        state: State<'_, Arc<LocalBotState>>,
    ) -> Result<(), String> {
        state.save(|c| c.mode = mode)?;
        refresh_window(&app);
        Ok(())
    }

    /// Opens a web address in the person's own browser. Sign-in pages such
    /// as Google's refuse to load inside an app window, so connecting Gmail
    /// signs in there and comes back to this app's local server. Only
    /// https addresses, so a page cannot launch anything else.
    #[tauri::command]
    pub fn open_external(url: String) -> Result<(), String> {
        if !url.starts_with("https://") || url.len() > 4096 || url.chars().any(char::is_control) {
            return Err("only https web addresses can be opened".into());
        }
        // Arguments go straight to the program (no shell), so `&` in the
        // address is safe.
        #[cfg(target_os = "windows")]
        let opened = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", &url])
            .spawn();
        #[cfg(target_os = "macos")]
        let opened = std::process::Command::new("open").arg(&url).spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let opened = std::process::Command::new("xdg-open").arg(&url).spawn();
        opened
            .map(|_| ())
            .map_err(|e| format!("could not open the browser: {e}"))
    }

    /// Forgets this computer's bot key (the bot stops; revoke it on the
    /// Bots page to disable it on the server too).
    #[tauri::command]
    pub async fn local_bot_forget(
        state: State<'_, Arc<LocalBotState>>,
    ) -> Result<LocalBotStatus, String> {
        // Best effort: release the chat route while the key still works.
        let _ = set_chat_route(&state.config(), false).await;
        stop_bot(&state);
        state.save(|c| *c = BotConfig::default())?;
        Ok(status_of(&state).await)
    }
}

/// How old a chat message may be before answering it would only confuse.
const CHAT_REPLY_MAX_AGE_MINUTES: i64 = 10;

/// Writes the next chat message as the agent the person was talking to,
/// using its character and the recent conversation the server sent along.
async fn answer_chat(
    engine: Engine,
    payload: &serde_json::Value,
    workdir: &std::path::Path,
) -> Result<String, String> {
    let age = payload
        .get("requested_at")
        .and_then(|v| v.as_str())
        .and_then(rfc3339_epoch_secs)
        .and_then(|then| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs() as i64;
            Some((now - then) / 60)
        });
    if age.is_some_and(|minutes| minutes > CHAT_REPLY_MAX_AGE_MINUTES) {
        return Err("the message waited too long while this computer was away".to_string());
    }
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let prompt = chat_prompt(&text("agent_name"), &text("system"), &text("history"));
    engines::run(engine, None, &prompt, workdir).await
}

/// The prompt for one chat reply, kept separate so it can be tested.
fn chat_prompt(agent_name: &str, system: &str, history: &str) -> String {
    let name = if agent_name.trim().is_empty() {
        "the assistant"
    } else {
        agent_name.trim()
    };
    format!(
        "{system}\n\nYou are {name}, replying in a chat on Airobot. The conversation so far, oldest first:\n\n{history}\n\nWrite {name}'s next message only, in character. No preamble, no quotes around it."
    )
}

/// Seconds since the epoch for the server's UTC timestamps
/// (`YYYY-MM-DDTHH:MM:SS[.fff]Z`), without a date library.
fn rfc3339_epoch_secs(s: &str) -> Option<i64> {
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let time = time.trim_end_matches('Z');
    let time = time.split(['+', '-']).next()?;
    let mut t = time.split(':');
    let h = t.next()?.parse::<i64>().ok()?;
    let mi = t.next()?.parse::<i64>().ok()?;
    let sec = t.next()?.split('.').next()?.parse::<i64>().ok()?;
    // Days from a civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + sec)
}

/// Polls the person's workspace for queued agent tasks, claims one at a
/// time, runs it through the chosen local AI tool and posts the real
/// outcome (a failure is reported as a failure, never a made-up result).
async fn run_local_bot_loop(state: Arc<LocalBotState>, generation: u64, workdir: PathBuf) {
    let current = |state: &LocalBotState| {
        state.enabled.load(Ordering::SeqCst)
            && state.generation.load(Ordering::SeqCst) == generation
    };
    let config = state.config();
    let Some(engine) = config.engine else { return };
    let (server, key) = (config.server_url.clone(), config.agent_key.clone());
    log::info!("[Local Bot] working through {:?} against {server}", engine);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap_or_default();

    while current(&state) {
        let _ = with_key(client.post(format!("{server}/agent/heartbeat")), &key)
            .json(&serde_json::json!({
                "node_name": config.agent_name,
                "status": "online",
                "capabilities": { "engine": engine }
            }))
            .send()
            .await;

        let tasks: Vec<serde_json::Value> =
            match with_key(client.get(format!("{server}/agent/tasks/poll")), &key)
                .send()
                .await
            {
                Ok(resp) if resp.status() == reqwest::StatusCode::UNAUTHORIZED => {
                    if let Ok(mut e) = state.last_error.lock() {
                        *e = Some("This computer's bot key was revoked. Pair it again.".into());
                    }
                    stop_bot(&state);
                    break;
                }
                Ok(resp) => resp
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .and_then(|b| b.get("queued_tasks").and_then(|t| t.as_array()).cloned())
                    .unwrap_or_default(),
                Err(_) => Vec::new(),
            };

        for task in tasks {
            if !current(&state) {
                break;
            }
            let Some(task_id) = task.get("id").and_then(|id| id.as_str()) else {
                continue;
            };
            let claimed = with_key(client.post(format!("{server}/tasks/{task_id}/claim")), &key)
                .json(&serde_json::json!({}))
                .send()
                .await
                .is_ok_and(|r| r.status().is_success());
            if !claimed {
                continue;
            }
            let title = task
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            if let Ok(mut w) = state.working_on.lock() {
                *w = Some(title.clone());
            }
            let payload = task.get("payload").cloned().unwrap_or_default();
            if payload.get("kind").and_then(|k| k.as_str()) == Some("chat_reply") {
                let outcome = answer_chat(engine, &payload, &workdir).await;
                let body = match &outcome {
                    Ok(text) => {
                        serde_json::json!({ "status": "completed", "output": { "result": text } })
                    }
                    Err(err) => {
                        serde_json::json!({ "status": "failed", "output": { "error": err } })
                    }
                };
                if let Ok(mut e) = state.last_error.lock() {
                    *e = outcome.err();
                }
                let _ = with_key(
                    client.post(format!("{server}/tasks/{task_id}/result")),
                    &key,
                )
                .json(&body)
                .send()
                .await;
                if let Ok(mut w) = state.working_on.lock() {
                    *w = None;
                }
                continue;
            }
            let prompt = format!(
                "You are an Airobot office bot completing a task for its owner. Reply with the finished result only.\n\nTask: {}\nDetails: {}",
                title,
                task.get("payload").unwrap_or(&serde_json::json!({}))
            );
            let outcome = engines::run(engine, None, &prompt, &workdir).await;
            let body = match &outcome {
                Ok(text) => serde_json::json!({
                    "status": "completed",
                    "output": { "result": text },
                    "notes": format!("Done on this computer with {:?}.", engine)
                }),
                Err(err) => serde_json::json!({
                    "status": "failed",
                    "output": { "error": err },
                    "notes": format!("The local AI tool could not finish this: {err}")
                }),
            };
            if let Ok(mut e) = state.last_error.lock() {
                *e = outcome.as_ref().err().cloned();
            }
            let _ = with_key(
                client.post(format!("{server}/tasks/{task_id}/result")),
                &key,
            )
            .json(&body)
            .send()
            .await;
            if let Ok(mut w) = state.working_on.lock() {
                *w = None;
            }
        }

        // Answering chats needs a quick turn-around; tasks can wait longer.
        let pause = if state.config().answer_chats { 3 } else { 10 };
        for _ in 0..pause {
            if !current(&state) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    log::info!("[Local Bot] stopped");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let bot_state = Arc::new(LocalBotState::default());

    tauri::Builder::default()
        // First, so a second launch hands over to the running app and exits:
        // two copies would show two tray icons and fight over one database.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = create_or_focus_window(app);
        }))
        .manage(Arc::clone(&bot_state))
        .manage(Arc::new(DesktopState::default()))
        .invoke_handler(tauri::generate_handler![
            commands::local_bot_status,
            commands::local_bot_pair,
            commands::local_bot_configure,
            commands::local_bot_set_chat,
            commands::local_bot_forget,
            commands::desktop_info,
            commands::desktop_set_mode,
            commands::open_external
        ])
        .setup(|app| {
            // Always on, so a problem on someone's computer can be read from
            // the log file (Windows: %LOCALAPPDATA%\com.airobot.desktop\logs,
            // macOS: ~/Library/Logs/com.airobot.desktop).
            app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(log::LevelFilter::Info)
                    .max_file_size(2_000_000)
                    .build(),
            )?;

            // Saved bot settings; a bot that was on keeps working after a restart.
            {
                let state = app.state::<Arc<LocalBotState>>();
                let path = app
                    .path()
                    .app_data_dir()
                    .map(|dir| dir.join("local-bot.json"))
                    .ok();
                if let Some(path) = &path {
                    if let Ok(mut c) = state.config.lock() {
                        *c = load_config(path);
                    }
                }
                if let Ok(mut p) = state.config_path.lock() {
                    *p = path;
                }
                let config = state.config();
                if config.enabled && !config.agent_key.is_empty() && config.engine.is_some() {
                    start_bot(&state, workdir(app.handle()));
                }
            }

            // This computer's own Airobot (database + server) starts in the
            // background; the window shows a loading page until it is ready.
            start_local(app.handle());
            create_or_focus_window(app.handle())?;

            // 1. Build System Tray Menu
            let open_item = MenuItem::with_id(app, "open", "Open Airobot", true, None::<&str>)?;
            let bot_on = app
                .state::<Arc<LocalBotState>>()
                .enabled
                .load(Ordering::SeqCst);
            let bot_item = MenuItem::with_id(
                app,
                "toggle_bot",
                if bot_on {
                    "Local AI Bot: ON"
                } else {
                    "Local AI Bot: OFF"
                },
                true,
                None::<&str>,
            )?;
            let in_cloud = app.state::<Arc<LocalBotState>>().config().mode == AppMode::Cloud;
            let mode_item = MenuItem::with_id(
                app,
                "switch_mode",
                if in_cloud {
                    "Work on this computer"
                } else {
                    "Switch to Airobot Cloud"
                },
                true,
                None::<&str>,
            )?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit Airobot", true, None::<&str>)?;
            let tray_menu =
                Menu::with_items(app, &[&open_item, &mode_item, &bot_item, &quit_item])?;

            // 2. Build System Tray Icon
            let mut tray = TrayIconBuilder::new();
            if let Some(icon) = app.default_window_icon().cloned() {
                tray = tray.icon(icon);
            }
            let _tray = tray
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .tooltip("Airobot")
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "open" => {
                        let _ = create_or_focus_window(app);
                    }
                    "toggle_bot" => {
                        let state = app.state::<Arc<LocalBotState>>();
                        if state.enabled.load(Ordering::SeqCst) {
                            stop_bot(&state);
                            let _ = state.save(|c| c.enabled = false);
                            let _ = bot_item.set_text("Local AI Bot: OFF");
                        } else {
                            let config = state.config();
                            if config.agent_key.is_empty() || config.engine.is_none() {
                                // Set up in the app: Connect → This computer.
                                let _ = create_or_focus_window(app);
                            } else {
                                let _ = state.save(|c| c.enabled = true);
                                start_bot(&state, workdir(app));
                                let _ = bot_item.set_text("Local AI Bot: ON");
                            }
                        }
                    }
                    "switch_mode" => {
                        let state = app.state::<Arc<LocalBotState>>();
                        let next = if state.config().mode == AppMode::Cloud {
                            AppMode::Local
                        } else {
                            AppMode::Cloud
                        };
                        let _ = state.save(|c| c.mode = next);
                        let _ = mode_item.set_text(if next == AppMode::Cloud {
                            "Work on this computer"
                        } else {
                            "Switch to Airobot Cloud"
                        });
                        let _ = create_or_focus_window(app);
                        refresh_window(app);
                    }
                    "quit" => {
                        log::info!(
                            "[Daemon] Quit requested from system tray. Terminating process."
                        );
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        let _ = create_or_focus_window(app);
                    }
                })
                .build(app)?;

            log::info!("[Daemon] Airobot desktop tray shell initialized successfully");
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| match event {
            // Closing the last window keeps Airobot in the tray; only Quit
            // (an explicit exit code) really ends it.
            tauri::RunEvent::ExitRequested { api, code, .. } if code.is_none() => {
                api.prevent_exit();
            }
            // Stop this computer's database cleanly on the way out.
            tauri::RunEvent::Exit => {
                let desktop = Arc::clone(&*app_handle.state::<Arc<DesktopState>>());
                tauri::async_runtime::block_on(async move {
                    let server = desktop.server.lock().await.take();
                    if let Some(server) = server {
                        server.stop().await;
                    }
                });
            }
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_urls_must_be_https_or_loopback() {
        assert_eq!(
            validate_server_url("https://airo-bot.com/").as_deref(),
            Ok("https://airo-bot.com")
        );
        assert!(validate_server_url("http://127.0.0.1:8081").is_ok());
        assert!(validate_server_url("http://evil.example").is_err());
        assert!(validate_server_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn server_timestamps_parse_to_epoch_seconds() {
        assert_eq!(rfc3339_epoch_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            rfc3339_epoch_secs("2000-03-01T00:00:00+00:00"),
            Some(951_868_800)
        );
        assert_eq!(
            rfc3339_epoch_secs("2026-11-14T19:31:22.123456Z"),
            Some(1_794_684_682)
        );
        assert_eq!(rfc3339_epoch_secs("nonsense"), None);
    }

    #[test]
    fn chat_prompts_carry_character_and_history() {
        let p = chat_prompt("Cactus", "You are Cactus, a calm plant.", "gardener: hi");
        assert!(p.starts_with("You are Cactus, a calm plant."));
        assert!(p.contains("gardener: hi"));
        assert!(p.contains("Write Cactus's next message only"));
        assert!(chat_prompt("", "", "x").contains("the assistant"));
    }

    #[test]
    fn the_window_url_is_always_a_valid_server() {
        assert!(validate_server_url(&app_url()).is_ok());
    }

    /// The window's pages are remote to Tauri (the local server on
    /// 127.0.0.1 and the hosted site), so a command that isn't declared in
    /// build.rs and granted in both capabilities is refused at run time and
    /// the settings panel silently never finds the AI tools. This keeps the
    /// handler list, build.rs and the capabilities in step.
    #[test]
    fn commands_are_granted_to_the_window() {
        let source = include_str!("lib.rs");
        let start = source.find("generate_handler![").expect("invoke handler");
        let end = start + source[start..].find(']').expect("end of handler list");
        let handled: Vec<&str> = source[start..end]
            .split(|c: char| c == ',' || c.is_whitespace() || c == '[')
            .filter_map(|t| t.strip_prefix("commands::"))
            .collect();
        assert!(!handled.is_empty());

        let build = include_str!("../build.rs");
        let capabilities = [
            ("default", include_str!("../capabilities/default.json")),
            ("remote", include_str!("../capabilities/remote.json")),
        ];
        let remote: serde_json::Value =
            serde_json::from_str(capabilities[1].1).expect("remote capability");
        let urls: Vec<&str> = remote["remote"]["urls"]
            .as_array()
            .expect("remote urls")
            .iter()
            .filter_map(|u| u.as_str())
            .collect();
        assert!(
            urls.contains(&"http://127.0.0.1:*"),
            "the local server's window must be allowed: {urls:?}"
        );

        // The window has no system title bar, so the page's own drag area
        // and buttons must be allowed to move, minimize, maximize and close it.
        for permission in [
            "core:window:allow-start-dragging",
            "core:window:allow-internal-toggle-maximize",
            "core:window:allow-minimize",
            "core:window:allow-toggle-maximize",
            "core:window:allow-close",
        ] {
            for (name, json) in capabilities {
                assert!(
                    json.contains(&format!("\"{permission}\"")),
                    "the frameless window needs {permission} in capabilities/{name}.json"
                );
            }
        }

        for command in &handled {
            assert!(
                build.contains(&format!("\"{command}\"")),
                "{command} is handled but not declared in build.rs COMMANDS"
            );
            let permission = format!("\"allow-{}\"", command.replace('_', "-"));
            for (name, json) in capabilities {
                assert!(
                    json.contains(&permission),
                    "{command} is not granted in capabilities/{name}.json ({permission})"
                );
            }
        }
    }
}
