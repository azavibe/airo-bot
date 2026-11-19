//! Local AI engines the desktop bot can work with: the official command-line
//! tools people already have signed in on this machine (Claude Code, Codex
//! for ChatGPT plans, OpenCode, Gemini CLI) and a local Ollama.
//!
//! How subscriptions are used, and what is never done:
//! - Each tool is run unmodified, as its maker publishes it, and signs itself
//!   in with the person's own account. Airobot never opens, copies, stores or
//!   sends those tools' credential files or tokens; sign-in state is asked of
//!   the tool itself (`claude auth status`, `codex login status`).
//! - The tools run only on this computer, only for tasks the signed-in
//!   person's own bot claims from their own workspace. The Airobot server
//!   never sees or uses anyone's subscription.
//! - Each run happens in an empty scratch folder with the tool's file and
//!   shell access off (Claude Code: no tools; Codex: read-only sandbox), so a
//!   task cannot change the person's files.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Longest a single task may run before the tool is stopped.
const RUN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Sign-in checks are quick; a hung tool must not hold up the settings panel.
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
/// Results are posted to the server; keep them to a sensible size.
const MAX_OUTPUT_CHARS: usize = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    Ollama,
    ClaudeCode,
    Codex,
    Opencode,
    Gemini,
}

impl Engine {
    pub const ALL: [Engine; 5] = [
        Engine::ClaudeCode,
        Engine::Codex,
        Engine::Opencode,
        Engine::Gemini,
        Engine::Ollama,
    ];

    fn binary(self) -> Option<&'static str> {
        match self {
            Engine::ClaudeCode => Some("claude"),
            Engine::Codex => Some("codex"),
            Engine::Opencode => Some("opencode"),
            Engine::Gemini => Some("gemini"),
            Engine::Ollama => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Engine::ClaudeCode => "Claude Code",
            Engine::Codex => "Codex (ChatGPT)",
            Engine::Opencode => "OpenCode",
            Engine::Gemini => "Gemini CLI",
            Engine::Ollama => "Ollama (local model)",
        }
    }

    fn plan(self) -> &'static str {
        match self {
            Engine::ClaudeCode => {
                "Your Claude Pro/Max plan or API key, as Claude Code is signed in"
            }
            Engine::Codex => "Your ChatGPT plan or OpenAI key, as Codex is signed in",
            Engine::Opencode => "The providers you connected in OpenCode",
            Engine::Gemini => "Your Google account or Gemini key, as Gemini CLI is signed in",
            Engine::Ollama => "Free, runs entirely on this computer",
        }
    }

    fn install_hint(self) -> &'static str {
        match self {
            Engine::ClaudeCode => "Install Claude Code, then run `claude` once to sign in.",
            Engine::Codex => "Install Codex (`npm i -g @openai/codex`), then run `codex login`.",
            Engine::Opencode => "Install OpenCode, then run `opencode auth login`.",
            Engine::Gemini => "Install Gemini CLI (`npm i -g @google/gemini-cli`), then run `gemini` once to sign in.",
            Engine::Ollama => "Install Ollama, start it, and pull a model, e.g. `ollama pull llama3`.",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineInfo {
    pub id: Engine,
    pub name: &'static str,
    pub plan: &'static str,
    pub installed: bool,
    /// `None` when the tool has no way to report its sign-in; it is then
    /// tried as-is and a sign-in problem shows up as that task's error.
    pub signed_in: Option<bool>,
    pub hint: &'static str,
    /// Models to choose from, where the tool can list them (Ollama's pulled
    /// models); empty when the tool picks or names its own.
    pub models: Vec<String>,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Where these tools' installers put their commands. An app started from
/// the Dock, the Start menu or an installer doesn't get the shell's PATH
/// (and keeps the PATH it started with while it sits in the tray), so these
/// are searched too, and added to the PATH the tools run with (npm installs
/// also need `node` from the same folders).
fn extra_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = home_dir();
    if let Some(home) = &home {
        for rel in [
            ".local/bin",
            ".claude/local",
            ".npm-global/bin",
            ".bun/bin",
            ".opencode/bin",
            ".volta/bin",
            ".cargo/bin",
            "bin",
        ] {
            dirs.push(home.join(rel));
        }
    }
    // A custom npm global folder (`npm config set prefix ...`).
    if let Some(prefix) = home
        .as_ref()
        .and_then(|h| std::fs::read_to_string(h.join(".npmrc")).ok())
        .and_then(|rc| npmrc_prefix(&rc))
    {
        dirs.push(if cfg!(windows) {
            prefix.clone()
        } else {
            prefix.join("bin")
        });
    }
    if cfg!(windows) {
        if let Some(appdata) = env_dir("APPDATA") {
            dirs.push(appdata.join("npm"));
        }
        if let Some(local) = env_dir("LOCALAPPDATA") {
            for rel in [
                r"Programs\claude",
                r"Programs\OpenAI\Codex\bin",
                r"Programs\Ollama",
                r"Microsoft\WinGet\Links",
                r"Microsoft\WindowsApps",
                "pnpm",
                r"pnpm\bin",
                r"Volta\bin",
            ] {
                dirs.push(local.join(rel));
            }
        }
        // nvm-windows keeps the active Node (and its global tools) here.
        if let Some(nvm) = env_dir("NVM_SYMLINK") {
            dirs.push(nvm);
        }
        dirs.push(PathBuf::from(r"C:\nvm4w\nodejs"));
        if let Some(pf) = env_dir("ProgramFiles") {
            dirs.push(pf.join("nodejs"));
        }
        if let Some(home) = &home {
            dirs.push(home.join(r"scoop\shims"));
        }
        if let Some(data) = env_dir("ProgramData") {
            dirs.push(data.join(r"chocolatey\bin"));
        }
    } else {
        if let Some(home) = &home {
            for rel in [
                ".local/share/mise/shims",
                ".asdf/shims",
                "Library/pnpm",
                ".local/share/pnpm",
                ".yarn/bin",
            ] {
                dirs.push(home.join(rel));
            }
            // nvm: every installed Node's global tools, newest first.
            if let Ok(entries) = std::fs::read_dir(home.join(".nvm/versions/node")) {
                let mut versions: Vec<PathBuf> =
                    entries.flatten().map(|e| e.path().join("bin")).collect();
                versions.sort();
                versions.reverse();
                dirs.extend(versions);
            }
        }
        for dir in [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/snap/bin",
        ] {
            dirs.push(PathBuf::from(dir));
        }
    }
    dirs
}

/// `prefix=...` from an `.npmrc`.
fn npmrc_prefix(rc: &str) -> Option<PathBuf> {
    rc.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "prefix")
            .then(|| value.trim().trim_matches('"'))
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    })
}

/// Expands `%NAME%` the way Windows does for `REG_EXPAND_SZ` values.
fn expand_windows_vars(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(value) if !name.is_empty() => out.push_str(&value),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The `Path` value from `reg query ... /v Path` output.
fn parse_reg_path(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let line = line.trim();
        if !line.to_ascii_lowercase().starts_with("path") {
            return None;
        }
        ["REG_EXPAND_SZ", "REG_SZ"]
            .iter()
            .find_map(|kind| line.split_once(kind))
            .map(|(_, value)| value.trim().to_string())
    })
}

async fn quiet_output(program: &str, args: &[&str], limit: Duration) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match tokio::time::timeout(limit, cmd.output()).await {
        Ok(Ok(out)) => Some(String::from_utf8_lossy(&out.stdout).into_owned()),
        _ => None,
    }
}

/// The PATH the person's own terminal would have right now. Windows: the
/// user and system PATH as saved in the registry (so a tool installed while
/// the app was running is found without restarting it). macOS/Linux: what
/// the login shell sets up (nvm, Homebrew, mise...), which apps opened from
/// the Dock never see.
async fn current_user_path() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(windows) {
        for key in [
            r"HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            r"HKCU\Environment",
        ] {
            if let Some(path) =
                quiet_output("reg", &["query", key, "/v", "Path"], Duration::from_secs(3))
                    .await
                    .and_then(|out| parse_reg_path(&out))
            {
                dirs.extend(
                    expand_windows_vars(&path)
                        .split(';')
                        .map(str::trim)
                        .filter(|d| !d.is_empty())
                        .map(PathBuf::from),
                );
            }
        }
    } else if let Some(shell) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) {
        let shell = shell.to_string_lossy().into_owned();
        const MARK: &str = "__AIROBOT_PATH__";
        let script = if shell.ends_with("fish") {
            format!("printf '{MARK}%s{MARK}' (string join : $PATH)")
        } else {
            format!("printf '{MARK}%s{MARK}' \"$PATH\"")
        };
        if let Some(out) = quiet_output(&shell, &["-ilc", &script], Duration::from_secs(4)).await {
            if let Some(path) = out.split(MARK).nth(1) {
                dirs.extend(std::env::split_paths(path));
            }
        }
    }
    dirs
}

/// How long a looked-up PATH is reused before asking again.
const PATH_CACHE_TTL: Duration = Duration::from_secs(20);

struct PathCache {
    at: std::time::Instant,
    process_path: std::ffi::OsString,
    dirs: Arc<Vec<PathBuf>>,
}

static PATH_CACHE: std::sync::Mutex<Option<PathCache>> = std::sync::Mutex::new(None);

fn same_dir(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy()
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(b.to_string_lossy().trim_end_matches('\\'))
    } else {
        a == b
    }
}

/// Every folder to look for the tools in: this process's PATH first, then
/// the person's current PATH, then the installers' usual folders.
async fn search_dirs() -> Arc<Vec<PathBuf>> {
    let process_path = std::env::var_os("PATH").unwrap_or_default();
    if let Ok(cache) = PATH_CACHE.lock() {
        if let Some(c) = cache.as_ref() {
            if c.process_path == process_path && c.at.elapsed() < PATH_CACHE_TTL {
                return Arc::clone(&c.dirs);
            }
        }
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    let candidates = std::env::split_paths(&process_path)
        .chain(current_user_path().await)
        .chain(extra_dirs());
    for dir in candidates {
        if dir.as_os_str().is_empty() || dirs.iter().any(|d| same_dir(d, &dir)) {
            continue;
        }
        dirs.push(dir);
    }
    let dirs = Arc::new(dirs);
    if let Ok(mut cache) = PATH_CACHE.lock() {
        *cache = Some(PathCache {
            at: std::time::Instant::now(),
            process_path,
            dirs: Arc::clone(&dirs),
        });
    }
    dirs
}

/// How to start a tool: the program, and arguments that go before the
/// tool's own (the script, when an npm shim is run through `node`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    pub program: PathBuf,
    pub prefix: Vec<PathBuf>,
}

impl Launcher {
    /// Windows can only run `.cmd`/`.bat` files through `cmd.exe`, which
    /// refuses multi-line arguments ("batch file arguments are invalid")
    /// and mangles quotes. Every prompt has several lines.
    fn is_batch(&self) -> bool {
        cfg!(windows)
            && self
                .program
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
    }
}

/// What an npm `.cmd` shim runs. npm writes one of two forms:
/// `"%dp0%\node_modules\pkg\bin\tool.exe" %*` (a native binary), or
/// `"%_prog%" "%dp0%\node_modules\pkg\bin\tool.js" %*` (a script for node).
fn npm_shim_target(shim: &str, dir: &Path) -> Option<PathBuf> {
    shim.lines().rev().find_map(|line| {
        let start = line
            .find("\"%dp0%\\")
            .map(|i| i + "\"%dp0%\\".len())
            .or_else(|| line.find("\"%~dp0\\").map(|i| i + "\"%~dp0\\".len()))?;
        let rest = &line[start..];
        let rel = &rest[..rest.find('"')?];
        (!rel.is_empty()).then(|| dir.join(rel))
    })
}

/// Turns an npm shim into a direct launch, so no `cmd.exe` is involved.
fn resolve_shim(shim: &Path, dirs: &[PathBuf]) -> Option<Launcher> {
    let text = std::fs::read_to_string(shim).ok()?;
    let dir = shim.parent()?;
    let target = npm_shim_target(&text, dir).filter(|t| t.is_file())?;
    let is_exe = target
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    if is_exe {
        return Some(Launcher {
            program: target,
            prefix: Vec::new(),
        });
    }
    // npm uses the node next to the shim when there is one.
    let local_node = dir.join("node.exe");
    let node = if local_node.is_file() {
        local_node
    } else {
        find_in(dirs, "node")?.program
    };
    Some(Launcher {
        program: node,
        prefix: vec![target],
    })
}

/// Finds a command in `dirs` the way a shell would. On Windows only files
/// Windows can start count (`.exe`, `.com`, `.cmd`, `.bat`; never npm's
/// extensionless shell scripts), and npm `.cmd` shims are run directly.
fn find_in(dirs: &[PathBuf], name: &str) -> Option<Launcher> {
    if !cfg!(windows) {
        return dirs
            .iter()
            .map(|d| d.join(name))
            .find(|p| p.is_file())
            .map(|program| Launcher {
                program,
                prefix: Vec::new(),
            });
    }
    for dir in dirs {
        for ext in ["exe", "com", "cmd", "bat"] {
            let path = dir.join(format!("{name}.{ext}"));
            if !path.is_file() {
                continue;
            }
            if ext == "cmd" || ext == "bat" {
                if let Some(direct) = resolve_shim(&path, dirs) {
                    return Some(direct);
                }
            }
            return Some(Launcher {
                program: path,
                prefix: Vec::new(),
            });
        }
    }
    None
}

/// The tool's command, found the way the person's terminal would find it
/// (plus the installers' usual folders).
pub async fn find_binary(name: &str) -> Option<Launcher> {
    let dirs = search_dirs().await;
    find_in(&dirs, name)
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The PATH the tools run with. A folder whose name contains the PATH
/// separator can't be listed and is left out, instead of the whole PATH
/// becoming empty.
fn path_env(dirs: &[PathBuf]) -> std::ffi::OsString {
    let usable: Vec<&PathBuf> = dirs
        .iter()
        .filter(|d| std::env::join_paths([d]).is_ok())
        .collect();
    std::env::join_paths(usable).unwrap_or_default()
}

async fn command(launcher: &Launcher, workdir: Option<&Path>) -> Command {
    let dirs = search_dirs().await;
    let mut cmd = Command::new(&launcher.program);
    cmd.args(&launcher.prefix)
        .env("PATH", path_env(&dirs))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = workdir {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    {
        // No console window flashing up for each run.
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Runs a sign-in check and returns its exit success and all its output.
async fn status_output(launcher: &Launcher, args: &[&str]) -> Option<(bool, String)> {
    let mut cmd = command(launcher, None).await;
    cmd.args(args);
    match tokio::time::timeout(STATUS_TIMEOUT, cmd.output()).await {
        Ok(Ok(out)) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.push('\n');
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            Some((out.status.success(), text))
        }
        Ok(Err(error)) => {
            log::warn!(
                "[Engines] could not run {}: {error}",
                launcher.program.display()
            );
            None
        }
        Err(_) => None,
    }
}

/// `claude auth status` prints `{"loggedIn": true, ...}`. A version without
/// that command (or any output it can't be read from) is "unknown", never
/// "signed out": the tool is still offered and a sign-in problem shows up
/// as the task's error.
fn claude_signed_in(output: &str) -> Option<bool> {
    let start = output.find('{')?;
    let end = output.rfind('}')?;
    let json: serde_json::Value = serde_json::from_str(output.get(start..=end)?).ok()?;
    json.get("loggedIn").and_then(|v| v.as_bool())
}

/// `codex login status`: "Logged in using ChatGPT" / "Not logged in".
fn codex_signed_in(success: bool, output: &str) -> Option<bool> {
    let lower = output.to_ascii_lowercase();
    if lower.contains("not logged in") {
        Some(false)
    } else if success && lower.contains("logged in") {
        Some(true)
    } else {
        None
    }
}

/// Where Ollama listens: `OLLAMA_HOST` when set (as `host`, `host:port` or
/// a URL), else the default port on this computer.
fn ollama_base() -> String {
    let raw = std::env::var("OLLAMA_HOST").unwrap_or_default();
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty() {
        return "http://127.0.0.1:11434".to_string();
    }
    let (scheme, rest) = match raw.split_once("://") {
        Some((scheme, rest)) => (scheme, rest),
        None => ("http", raw),
    };
    // Listening on every address means it is reachable on loopback.
    let rest = rest
        .replacen("0.0.0.0", "127.0.0.1", 1)
        .replacen("[::]", "127.0.0.1", 1);
    let has_port = rest
        .rsplit_once(':')
        .is_some_and(|(_, port)| !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()));
    if has_port {
        format!("{scheme}://{rest}")
    } else {
        format!("{scheme}://{rest}:11434")
    }
}

/// The models Ollama has, or `None` when it isn't answering.
async fn ollama_models() -> Option<Vec<String>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(1500))
        .build()
        .ok()?;
    let body: serde_json::Value = client
        .get(format!("{}/api/tags", ollama_base()))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    Some(
        body.get("models")
            .and_then(|m| m.as_array())
            .map(|models| {
                models
                    .iter()
                    .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// Prefers Llama 3 when it is there, else the first model pulled.
fn pick_ollama_model(models: &[String]) -> Option<String> {
    models
        .iter()
        .find(|m| m.starts_with("llama3"))
        .or_else(|| models.first())
        .cloned()
}

/// Whether the tool is on this computer (cheap: no sign-in check).
pub async fn installed(engine: Engine) -> bool {
    match engine.binary() {
        Some(name) => find_binary(name).await.is_some(),
        None => ollama_models().await.is_some() || find_binary("ollama").await.is_some(),
    }
}

pub async fn detect(engine: Engine) -> EngineInfo {
    let mut models = Vec::new();
    let (installed, signed_in) = match engine {
        // Running with a model pulled is "ready"; installed but stopped (or
        // with no model yet) is shown as installed but not ready.
        Engine::Ollama => match ollama_models().await {
            Some(pulled) => {
                let ready = !pulled.is_empty();
                models = pulled;
                (true, Some(ready))
            }
            None => (find_binary("ollama").await.is_some(), Some(false)),
        },
        _ => match engine.binary() {
            None => (false, Some(false)),
            Some(name) => match find_binary(name).await {
                None => (false, Some(false)),
                Some(bin) => {
                    let status = match engine {
                        Engine::ClaudeCode => status_output(&bin, &["auth", "status"])
                            .await
                            .and_then(|(_, out)| claude_signed_in(&out)),
                        Engine::Codex => status_output(&bin, &["login", "status"])
                            .await
                            .and_then(|(ok, out)| codex_signed_in(ok, &out)),
                        _ => None,
                    };
                    (true, status)
                }
            },
        },
    };
    EngineInfo {
        id: engine,
        name: engine.label(),
        plan: engine.plan(),
        installed,
        signed_in,
        hint: engine.install_hint(),
        models,
    }
}

/// All the tools at once (each check can take a moment).
pub async fn detect_all() -> Vec<EngineInfo> {
    let [a, b, c, d, e] = Engine::ALL;
    let (a, b, c, d, e) = tokio::join!(detect(a), detect(b), detect(c), detect(d), detect(e));
    let found = vec![a, b, c, d, e];
    log::info!(
        "[Engines] {}",
        found
            .iter()
            .map(|e| format!(
                "{}={}",
                e.name,
                match (e.installed, e.signed_in) {
                    (false, _) => "missing",
                    (true, Some(true)) => "ready",
                    (true, Some(false)) => "not signed in/not running",
                    (true, None) => "installed",
                }
            ))
            .collect::<Vec<_>>()
            .join(", ")
    );
    found
}

/// Longest prompt passed on a command line: Windows allows 32,767
/// characters for the whole line (UTF-16 units, quoting included).
const MAX_ARG_PROMPT_CHARS: usize = if cfg!(windows) { 24_000 } else { 200_000 };

/// Keeps the start (character and rules) and the end (the latest messages
/// and what to answer) of a prompt that is too long for a command line.
fn fit_command_line(prompt: &str) -> String {
    let count = prompt.chars().count();
    if count <= MAX_ARG_PROMPT_CHARS {
        return prompt.to_string();
    }
    let marker = "

[... earlier conversation shortened ...]

";
    let head = MAX_ARG_PROMPT_CHARS / 5;
    let tail = MAX_ARG_PROMPT_CHARS - head - marker.chars().count();
    let start: String = prompt.chars().take(head).collect();
    let end: String = prompt.chars().skip(count - tail).collect();
    format!("{start}{marker}{end}")
}

fn clip(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX_OUTPUT_CHARS {
        trimmed.to_string()
    } else {
        let cut: String = trimmed.chars().take(MAX_OUTPUT_CHARS).collect();
        format!("{cut}\n\n[output truncated]")
    }
}

/// The last lines of a failed run's error output, for the task's error.
fn short_error(stderr: &[u8], fallback: &str) -> String {
    let text = String::from_utf8_lossy(stderr);
    let tail: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(3)
        .collect();
    if tail.is_empty() {
        fallback.to_string()
    } else {
        tail.into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(400)
            .collect()
    }
}

/// A model name as a tool's `--model` value. Never something that could
/// read as another flag, and one word.
fn usable_model(model: Option<&str>) -> Option<&str> {
    model.map(str::trim).filter(|m| {
        !m.is_empty()
            && !m.starts_with('-')
            && m.len() <= 200
            && !m.chars().any(|c| c.is_whitespace() || c.is_control())
    })
}

/// Runs one task's prompt through the chosen engine and returns its answer.
/// `model` picks the model inside the tool (`None`: the tool's default).
/// `workdir` is an empty scratch folder owned by the app.
pub async fn run(
    engine: Engine,
    model: Option<&str>,
    prompt: &str,
    workdir: &Path,
) -> Result<String, String> {
    run_with(engine, model, prompt, workdir, false).await
}

/// Claude Code's tools that stay off even with web access: no shell, no
/// file reads or writes, no sub-agents. Reading files plus the web would let
/// a web page steer a bot into sending local files out.
const CLAUDE_TOOLS_OFF_WITH_WEB: &str =
    "Bash,Edit,Write,MultiEdit,NotebookEdit,NotebookRead,Read,Glob,Grep,LS,Task,TodoWrite";

/// [`run`], with the tool's web search and page reading on when `web` is
/// set (Claude Code: WebSearch and WebFetch only; Codex: live search in its
/// read-only sandbox). Other tools ignore it.
pub async fn run_with(
    engine: Engine,
    model: Option<&str>,
    prompt: &str,
    workdir: &Path,
    web: bool,
) -> Result<String, String> {
    let model = usable_model(model);
    if engine == Engine::Ollama {
        return run_ollama(model, prompt).await;
    }
    let not_installed = || format!("{} is not installed on this computer.", engine.label());
    let bin = find_binary(engine.binary().ok_or_else(not_installed)?)
        .await
        .ok_or_else(not_installed)?;
    std::fs::create_dir_all(workdir).map_err(|e| format!("cannot prepare a work folder: {e}"))?;

    let last_message = workdir.join("codex-last-message.txt");
    let mut cmd = command(&bin, Some(workdir)).await;
    // OpenCode and Gemini take the prompt as an argument; Windows limits a
    // command line to 32,767 characters.
    let arg_prompt = fit_command_line(prompt);
    let mut stdin_prompt = true;
    match engine {
        Engine::ClaudeCode => {
            // Print mode reads the prompt from stdin. "*" removes every tool,
            // so the run can only answer, never read or change files.
            if web {
                cmd.args([
                    "-p",
                    "--output-format",
                    "json",
                    "--allowedTools",
                    "WebSearch,WebFetch",
                    "--disallowedTools",
                    CLAUDE_TOOLS_OFF_WITH_WEB,
                ]);
            } else {
                cmd.args(["-p", "--output-format", "json", "--disallowedTools", "*"]);
            }
            if let Some(m) = model {
                cmd.args(["--model", m]);
            }
        }
        Engine::Codex => {
            // `-` reads the prompt from stdin; read-only sandbox; nothing
            // persisted to Codex's history; final answer written to a file.
            let _ = std::fs::remove_file(&last_message);
            cmd.arg("exec");
            if let Some(m) = model {
                cmd.args(["-m", m]);
            }
            if web {
                cmd.args(["-c", "web_search=live"]);
            }
            cmd.args([
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
                "--ephemeral",
                "-o",
            ])
            .arg(&last_message)
            .arg("-");
        }
        Engine::Opencode => {
            cmd.arg("run");
            if let Some(m) = model {
                cmd.args(["-m", m]);
            }
            cmd.arg(&arg_prompt);
            stdin_prompt = false;
        }
        Engine::Gemini => {
            if let Some(m) = model {
                cmd.args(["-m", m]);
            }
            cmd.arg("-p").arg(&arg_prompt);
            stdin_prompt = false;
        }
        Engine::Ollama => unreachable!(),
    }
    if stdin_prompt {
        cmd.stdin(Stdio::piped());
    } else if bin.is_batch() {
        return Err(format!(
            "{} is installed as {}, which Windows can't pass a task to. Reinstall it with its official installer or npm.",
            engine.label(),
            bin.program.display()
        ));
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", engine.label()))?;
    if stdin_prompt {
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(prompt.as_bytes())
                .await
                .map_err(|e| format!("could not send the task to {}: {e}", engine.label()))?;
            // Closing stdin tells the tool the prompt is complete.
            drop(stdin);
        }
    }
    let out = match tokio::time::timeout(RUN_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(format!("{} stopped unexpectedly: {e}", engine.label())),
        Err(_) => {
            return Err(format!(
                "{} took longer than 10 minutes and was stopped.",
                engine.label()
            ))
        }
    };

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    match engine {
        Engine::ClaudeCode => {
            // On a failed run (e.g. signed out) the reason is the result text.
            let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
                .map_err(|_| short_error(&out.stderr, "Claude Code returned no answer."))?;
            let result = parsed
                .get("result")
                .and_then(|r| r.as_str())
                .unwrap_or_default();
            let is_error = parsed
                .get("is_error")
                .and_then(|e| e.as_bool())
                .unwrap_or(false);
            if !out.status.success() || is_error || result.trim().is_empty() {
                return Err(if result.trim().is_empty() {
                    short_error(&out.stderr, "Claude Code returned no answer.")
                } else {
                    format!(
                        "Claude Code: {}",
                        result.trim().chars().take(400).collect::<String>()
                    )
                });
            }
            Ok(clip(result))
        }
        Engine::Codex => {
            let answer = std::fs::read_to_string(&last_message).unwrap_or_default();
            let _ = std::fs::remove_file(&last_message);
            if !out.status.success() || answer.trim().is_empty() {
                return Err(short_error(&out.stderr, "Codex returned no answer."));
            }
            Ok(clip(&answer))
        }
        _ => {
            if !out.status.success() || stdout.trim().is_empty() {
                return Err(short_error(
                    &out.stderr,
                    &format!("{} returned no answer.", engine.label()),
                ));
            }
            Ok(clip(&stdout))
        }
    }
}

async fn run_ollama(chosen: Option<&str>, prompt: &str) -> Result<String, String> {
    let models = ollama_models()
        .await
        .ok_or_else(|| "Ollama is not running on this computer.".to_string())?;
    let model = match chosen {
        Some(m)
            if models
                .iter()
                .any(|have| have == m || have.split(':').next() == Some(m)) =>
        {
            m.to_string()
        }
        Some(m) => {
            return Err(format!(
                "Ollama doesn't have the model {m}. Pull it first: `ollama pull {m}`."
            ))
        }
        None => pick_ollama_model(&models).ok_or_else(|| {
            "Ollama has no model yet. Pull one first, e.g. `ollama pull llama3`.".to_string()
        })?,
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    let body: serde_json::Value = client
        .post(format!("{}/api/generate", ollama_base()))
        .json(&serde_json::json!({ "model": model, "prompt": prompt, "stream": false }))
        .send()
        .await
        .map_err(|_| "Ollama is not running on this computer.".to_string())?
        .json()
        .await
        .map_err(|_| "Ollama returned an unreadable answer.".to_string())?;
    body.get("response")
        .and_then(|r| r.as_str())
        .map(clip)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "Ollama returned no answer.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_round_trip_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&Engine::ClaudeCode).unwrap(),
            "\"claude_code\""
        );
        let e: Engine = serde_json::from_str("\"codex\"").unwrap();
        assert_eq!(e, Engine::Codex);
    }

    #[test]
    fn long_output_is_clipped() {
        let long = "x".repeat(MAX_OUTPUT_CHARS + 10);
        assert!(clip(&long).ends_with("[output truncated]"));
        assert_eq!(clip("  hi  "), "hi");
    }

    #[test]
    fn errors_keep_the_last_lines() {
        assert_eq!(
            short_error(b"one\n\ntwo\nthree\nfour\n", "x"),
            "two three four"
        );
        assert_eq!(short_error(b"", "fallback"), "fallback");
    }

    /// Fake `claude` and `codex` commands on PATH record what Airobot runs
    /// and answer the way the real tools do, so the exact flags, the prompt
    /// on stdin, the scratch folder and the answer parsing are all checked.
    #[cfg(unix)]
    #[tokio::test]
    async fn runs_the_cli_tools_with_tools_off_and_reads_their_answers() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("airobot-engine-test-{}", std::process::id()));
        let bin = root.join("bin");
        let work = root.join("work");
        std::fs::create_dir_all(&bin).unwrap();
        let log = root.join("args.log");
        let fake = |name: &str, body: &str| {
            let path = bin.join(name);
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\necho \"{name} $*\" >> '{}'\npwd >> '{}'\n{body}\n",
                    log.display(),
                    log.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        fake(
            "claude",
            r#"if [ "$1" = auth ]; then echo '{"loggedIn": true, "authMethod": "claude.ai"}'; exit 0; fi
prompt=$(cat)
case "$prompt" in *FAILME*) echo '{"is_error":true,"result":"Not logged in"}'; exit 1;; esac
printf '{"type":"result","is_error":false,"result":"claude says: %s"}' "$prompt""#,
        );
        fake(
            "codex",
            r#"if [ "$1" = login ]; then echo "Not logged in"; exit 1; fi
while [ $# -gt 0 ]; do if [ "$1" = -o ]; then out="$2"; fi; shift; done
prompt=$(cat)
printf 'codex says: %s' "$prompt" > "$out""#,
        );
        let old_path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![bin.clone()];
        paths.extend(std::env::split_paths(&old_path));
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());

        let claude = detect(Engine::ClaudeCode).await;
        assert!(claude.installed);
        assert_eq!(claude.signed_in, Some(true));
        let codex = detect(Engine::Codex).await;
        assert_eq!(
            codex.signed_in,
            Some(false),
            "signed-out Codex is reported, not guessed"
        );

        assert_eq!(
            run(Engine::ClaudeCode, Some("opus"), "hello", &work)
                .await
                .unwrap(),
            "claude says: hello"
        );
        let err = run(Engine::ClaudeCode, None, "FAILME", &work)
            .await
            .unwrap_err();
        assert!(err.contains("Not logged in"), "{err}");
        assert_eq!(
            run(Engine::Codex, None, "hi there", &work).await.unwrap(),
            "codex says: hi there"
        );
        // With web access: only the web tools, and Codex searches live.
        assert_eq!(
            run_with(Engine::ClaudeCode, None, "web please", &work, true)
                .await
                .unwrap(),
            "claude says: web please"
        );
        assert_eq!(
            run_with(Engine::Codex, None, "web too", &work, true)
                .await
                .unwrap(),
            "codex says: web too"
        );

        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(
            calls.contains("claude -p --output-format json --disallowedTools * --model opus"),
            "{calls}"
        );
        assert!(
            calls.contains("codex exec --skip-git-repo-check --sandbox read-only --ephemeral -o"),
            "{calls}"
        );
        assert!(
            calls.contains(&format!(
                "claude -p --output-format json --allowedTools WebSearch,WebFetch --disallowedTools {CLAUDE_TOOLS_OFF_WITH_WEB}"
            )),
            "{calls}"
        );
        assert!(
            calls.contains("codex exec -c web_search=live --skip-git-repo-check"),
            "{calls}"
        );
        assert!(
            calls.contains(work.to_str().unwrap()),
            "runs happen in the scratch folder"
        );
        // The prompt went over stdin, never on the command line.
        assert!(!calls.contains("hello"));

        std::env::set_var("PATH", old_path);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn npm_shims_are_read_in_both_forms() {
        let dir = Path::new(r"C:\npm");
        let native = "@ECHO off\r\nGOTO start\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\"%dp0%\\node_modules\\@opencode\\cli\\bin\\opencode.exe\"   %*\r\n";
        assert_eq!(
            npm_shim_target(native, dir),
            Some(dir.join(r"node_modules\@opencode\cli\bin\opencode.exe"))
        );
        let script = "endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n";
        assert_eq!(
            npm_shim_target(script, dir),
            Some(dir.join(r"node_modules\@openai\codex\bin\codex.js"))
        );
        let old = "@\"%~dp0\\node_modules\\x\\cli.js\" %*";
        assert_eq!(
            npm_shim_target(old, dir),
            Some(dir.join(r"node_modules\x\cli.js"))
        );
        assert_eq!(npm_shim_target("@echo hi", dir), None);
    }

    #[test]
    fn the_saved_windows_path_is_read_and_expanded() {
        let out = "\r\nHKEY_CURRENT_USER\\Environment\r\n    Path    REG_EXPAND_SZ    %AIROBOT_TEST_HOME%\\.local\\bin;C:\\tools\r\n\r\n";
        assert_eq!(
            parse_reg_path(out).as_deref(),
            Some(r"%AIROBOT_TEST_HOME%\.local\bin;C:\tools")
        );
        assert_eq!(
            parse_reg_path("    Path    REG_SZ    C:\\a;C:\\b").as_deref(),
            Some(r"C:\a;C:\b")
        );
        assert_eq!(parse_reg_path("ERROR: not found"), None);
        std::env::set_var("AIROBOT_TEST_HOME", r"C:\Users\me");
        assert_eq!(
            expand_windows_vars(r"%AIROBOT_TEST_HOME%\bin;%AIROBOT_NOT_SET%\x;50%"),
            r"C:\Users\me\bin;%AIROBOT_NOT_SET%\x;50%"
        );
    }

    #[test]
    fn sign_in_checks_never_guess_signed_out() {
        assert_eq!(
            claude_signed_in("{\n  \"loggedIn\": true,\n  \"authMethod\": \"claude.ai\"\n}"),
            Some(true)
        );
        assert_eq!(claude_signed_in("{\"loggedIn\": false}"), Some(false));
        // An older Claude Code without `auth status` is "unknown", so the
        // tool is still offered.
        assert_eq!(claude_signed_in("error: unknown command 'auth'"), None);
        assert_eq!(codex_signed_in(true, "Logged in using ChatGPT"), Some(true));
        assert_eq!(codex_signed_in(false, "Not logged in"), Some(false));
        assert_eq!(
            codex_signed_in(false, "error: unrecognized subcommand"),
            None
        );
    }

    #[test]
    fn ollama_host_settings_are_understood() {
        // One test touches OLLAMA_HOST, so they can't race each other.
        let old = std::env::var_os("OLLAMA_HOST");
        for (value, expected) in [
            ("", "http://127.0.0.1:11434"),
            ("0.0.0.0", "http://127.0.0.1:11434"),
            ("0.0.0.0:8000", "http://127.0.0.1:8000"),
            ("http://gpu-box:11434/", "http://gpu-box:11434"),
            ("localhost", "http://localhost:11434"),
        ] {
            std::env::set_var("OLLAMA_HOST", value);
            assert_eq!(ollama_base(), expected, "OLLAMA_HOST={value}");
        }
        match old {
            Some(v) => std::env::set_var("OLLAMA_HOST", v),
            None => std::env::remove_var("OLLAMA_HOST"),
        }
        let models = vec!["qwen2.5:7b".to_string(), "llama3.1:8b".to_string()];
        assert_eq!(pick_ollama_model(&models).as_deref(), Some("llama3.1:8b"));
        assert_eq!(
            pick_ollama_model(&["qwen2.5:7b".to_string()]).as_deref(),
            Some("qwen2.5:7b")
        );
        assert_eq!(pick_ollama_model(&[]), None);
    }

    #[test]
    fn long_prompts_fit_a_command_line() {
        let short = "hello\nthere";
        assert_eq!(fit_command_line(short), short);
        let long = format!("RULES{}LATEST", "x".repeat(MAX_ARG_PROMPT_CHARS * 2));
        let fitted = fit_command_line(&long);
        assert!(fitted.chars().count() <= MAX_ARG_PROMPT_CHARS);
        assert!(fitted.starts_with("RULES") && fitted.ends_with("LATEST"));
    }

    #[test]
    fn a_model_never_becomes_another_flag() {
        assert_eq!(usable_model(Some(" opus ")), Some("opus"));
        assert_eq!(usable_model(Some("llama3.1:8b")), Some("llama3.1:8b"));
        assert_eq!(usable_model(Some("--dangerously-skip-permissions")), None);
        assert_eq!(usable_model(Some("gpt 5")), None);
        assert_eq!(usable_model(Some("")), None);
        assert_eq!(usable_model(None), None);
    }

    #[test]
    fn npmrc_prefix_is_found() {
        assert_eq!(
            npmrc_prefix("registry=https://r\nprefix = \"C:\\npm-global\"\n"),
            Some(PathBuf::from(r"C:\npm-global"))
        );
        assert_eq!(npmrc_prefix("registry=https://r"), None);
    }

    /// The Windows install shapes that kept the tools from being found or
    /// run: npm's extensionless shell script next to its `.cmd` shim, and a
    /// `.cmd` shim that runs a script through node (Windows refuses to pass
    /// a multi-line prompt to a `.cmd`). A fake OpenCode installed the npm
    /// way must be found and answer a multi-line, quoted prompt exactly.
    #[cfg(windows)]
    #[tokio::test]
    async fn npm_installed_tools_are_found_and_run_on_windows() {
        let Some(node) = find_binary("node").await else {
            eprintln!("node not installed; skipping");
            return;
        };
        let root = std::env::temp_dir().join(format!("airobot-npm-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("npm");
        let pkg = bin.join(r"node_modules\fake-opencode\bin");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("opencode.js"),
            "process.stdout.write('opencode got: ' + JSON.stringify(process.argv.slice(2)));",
        )
        .unwrap();
        // What npm writes: a shell script (useless on Windows) and a shim.
        std::fs::write(bin.join("opencode"), "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::write(
            bin.join("opencode.cmd"),
            "@ECHO off\r\nSETLOCAL\r\nIF EXIST \"%dp0%\\node.exe\" (SET \"_prog=%dp0%\\node.exe\") ELSE (SET \"_prog=node\")\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\fake-opencode\\bin\\opencode.js\" %*\r\n",
        )
        .unwrap();

        let dirs = vec![bin.clone(), node.program.parent().unwrap().to_path_buf()];
        let found = find_in(&dirs, "opencode").expect("found through its shim");
        assert_eq!(found.program, node.program, "run with node, not cmd.exe");
        assert_eq!(found.prefix, vec![pkg.join("opencode.js")]);

        let old_path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![bin.clone()];
        paths.extend(std::env::split_paths(&old_path));
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        let info = detect(Engine::Opencode).await;
        assert!(info.installed);
        let prompt = "You are \"Cactus\".\n\nSay hi & 100% done";
        let answer = run(
            Engine::Opencode,
            Some("anthropic/claude-test"),
            prompt,
            &root.join("work"),
        )
        .await;
        std::env::set_var("PATH", old_path);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            answer.unwrap(),
            format!(
                "opencode got: {}",
                serde_json::to_string(&vec!["run", "-m", "anthropic/claude-test", prompt]).unwrap()
            )
        );
    }

    /// What this computer really has, as the app would see it:
    /// `cargo test --lib -- --ignored --nocapture what_this_computer_has`
    #[tokio::test]
    #[ignore = "reads the real tools on this computer"]
    async fn what_this_computer_has() {
        for engine in Engine::ALL {
            let launcher = match engine.binary() {
                Some(name) => find_binary(name).await,
                None => find_binary("ollama").await,
            };
            let info = detect(engine).await;
            println!(
                "{:<22} installed={:<5} signed_in={:<11} via {:?}",
                info.name,
                info.installed,
                format!("{:?}", info.signed_in),
                launcher
            );
        }
    }

    #[test]
    fn a_missing_tool_is_reported_not_installed() {
        let dirs = vec![std::env::temp_dir()];
        assert!(find_in(&dirs, "airobot-no-such-tool-xyz").is_none());
    }
}
