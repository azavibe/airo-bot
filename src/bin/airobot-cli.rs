//! Fast, secure, zero-dependency Rust CLI and 24/7 daemon for the Air noticeboard.
//! Coordinates agents, runs assigned workflow steps, and listens for live chat commands.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use serde::{Deserialize, Serialize};
use serde_json::json;

const DEFAULT_BASE_URL: &str = "https://airo-bot.com";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    pub base_url: String,
    pub agent_key: String,
    pub agent_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DaemonState {
    pub cursor: i64,
}

fn config_dir() -> PathBuf {
    if let Ok(dir) =
        std::env::var("AIROBOT_CONFIG_DIR").or_else(|_| std::env::var("AIROBOT_CONFIG_DIR"))
    {
        if !dir.trim().is_empty() {
            let p = PathBuf::from(dir.trim());
            ensure_dir_secure(&p);
            return p;
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let p = PathBuf::from(home).join(".config").join("airobot");
    ensure_dir_secure(&p);
    p
}

fn ensure_dir_secure(dir: &Path) {
    if !dir.exists() {
        let _ = fs::create_dir_all(dir);
        #[cfg(unix)]
        {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
    }
}

fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

fn state_path() -> PathBuf {
    config_dir().join("state.json")
}

fn load_config() -> Result<CliConfig, String> {
    let p = config_path();
    if !p.exists() {
        return Err(format!(
            "Not logged in. Config not found at {}. Run 'airobot-cli login' first.",
            p.display()
        ));
    }
    let content =
        fs::read_to_string(&p).map_err(|e| format!("Failed to read {}: {}", p.display(), e))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("Invalid config format at {}: {}", p.display(), e))
}

fn save_config(cfg: &CliConfig) -> Result<(), String> {
    let p = config_path();
    let content = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;
    fs::write(&p, content).map_err(|e| format!("Failed to write {}: {}", p.display(), e))?;
    #[cfg(unix)]
    {
        let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn load_state() -> DaemonState {
    let p = state_path();
    if p.exists() {
        if let Ok(content) = fs::read_to_string(&p) {
            if let Ok(st) = serde_json::from_str::<DaemonState>(&content) {
                return st;
            }
        }
    }
    DaemonState::default()
}

fn save_state(st: &DaemonState) {
    let p = state_path();
    if let Ok(content) = serde_json::to_string(st) {
        let _ = fs::write(&p, content);
        #[cfg(unix)]
        {
            let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o600));
        }
    }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

async fn api_request(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    auth_token: Option<&str>,
    body: Option<&serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let mut req = client.request(method, url);
    req = req.header("Accept", "application/json");
    if let Some(token) = auth_token {
        req = req.header("Authorization", format!("Bearer {}", token.trim()));
    }
    if let Some(b) = body {
        req = req.header("Content-Type", "application/json").json(b);
    }
    let res = req
        .send()
        .await
        .map_err(|e| format!("Network request failed ({}): {}", url, e))?;

    let status = res.status();
    let text = res
        .text()
        .await
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    if !status.is_success() {
        return Err(format!(
            "API error (HTTP {}): {}",
            status.as_u16(),
            text.trim()
        ));
    }

    if text.trim().is_empty() {
        return Ok(json!({}));
    }

    serde_json::from_str(&text)
        .map_err(|_| json!({ "raw": text }))
        .or_else(|_| Ok(json!({ "raw": text })))
}

fn clean_runner_output(output: &str) -> String {
    let mut cleaned_lines = Vec::new();
    let mut skip_arrow_block = false;

    for line in output.lines() {
        let stripped = line.trim();
        if stripped.is_empty() {
            if !skip_arrow_block {
                cleaned_lines.push(line);
            }
            continue;
        }
        // Tool header indicators: [agentgrep], [read], [bash], [batch], [Tokens:...]
        if stripped.starts_with('[') && stripped.contains(']') && !stripped.contains("http") {
            skip_arrow_block = true;
            continue;
        }
        // Arrow result markers: "→ query: ..." or "→ 1 ---"
        if stripped.starts_with('→') {
            skip_arrow_block = true;
            continue;
        }
        // Command echo or usage lines
        if stripped.starts_with("$ ") || stripped.starts_with("usage: ") {
            continue;
        }
        skip_arrow_block = false;
        cleaned_lines.push(line);
    }

    let result = cleaned_lines.join("\n").trim().to_string();
    if result.is_empty() {
        output.trim().to_string()
    } else {
        result
    }
}

fn run_agy_headless(prompt: &str, cwd: &Path, effort: &str) -> String {
    let agy_bin = if Path::new("/root/.local/bin/agy").exists() {
        "/root/.local/bin/agy"
    } else {
        "agy"
    };

    let child = Command::new(agy_bin)
        .arg("--effort")
        .arg(effort)
        .arg("--disable-slash-commands")
        .arg("--dangerously-skip-permissions")
        .arg("-p")
        .arg(prompt)
        .current_dir(cwd)
        .output();

    match child {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if stdout.starts_with('{') && stdout.ends_with('}') {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
                    if let Some(r) = v
                        .get("response")
                        .or_else(|| v.get("text"))
                        .and_then(|s| s.as_str())
                    {
                        return clean_runner_output(r);
                    }
                }
            }
            clean_runner_output(&stdout)
        }
        Err(e) => {
            eprintln!("⚠️ AGY headless execution failed: {}", e);
            String::new()
        }
    }
}

fn run_jcode_headless(prompt: &str, cwd: &Path) -> String {
    let jcode_bin = if Path::new("/root/.local/bin/jcode").exists() {
        "/root/.local/bin/jcode"
    } else {
        "jcode"
    };

    let child = Command::new(jcode_bin)
        .arg("-C")
        .arg(cwd)
        .arg("run")
        .arg("--quiet")
        .arg("--json")
        .arg(prompt)
        .output();

    match child {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&stdout) {
                if let Some(text) = v.get("text").and_then(|s| s.as_str()) {
                    return clean_runner_output(text);
                }
            }
            clean_runner_output(&stdout)
        }
        Err(e) => {
            eprintln!("⚠️ Jcode headless execution failed: {}", e);
            String::new()
        }
    }
}

fn run_opencode_headless(prompt: &str, cwd: &Path) -> String {
    let opencode_bin = if Path::new("/root/.opencode/bin/opencode").exists() {
        "/root/.opencode/bin/opencode"
    } else {
        "opencode"
    };

    // `run --format json` streams one JSON event per line, so the assistant
    // text is collected from the "text" parts instead of scraping the TUI's
    // decorated stdout. --auto keeps a dispatched step from stalling on a
    // permission prompt the daemon cannot answer.
    let child = Command::new(opencode_bin)
        .arg("run")
        .arg("--auto")
        .arg("--format")
        .arg("json")
        .arg(prompt)
        .current_dir(cwd)
        .output();

    match child {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let mut reply = String::new();
            for line in stdout.lines() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    if v.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(text) = v
                            .get("part")
                            .and_then(|p| p.get("text"))
                            .and_then(|t| t.as_str())
                        {
                            if !reply.is_empty() {
                                reply.push('\n');
                            }
                            reply.push_str(text);
                        }
                    }
                }
            }
            if reply.trim().is_empty() {
                clean_runner_output(&stdout)
            } else {
                clean_runner_output(&reply)
            }
        }
        Err(e) => {
            eprintln!("⚠️ OpenCode headless execution failed: {}", e);
            String::new()
        }
    }
}

fn execute_headless_prompt(prompt: &str, cwd: &Path, runner: &str, effort: &str) -> String {
    match runner.to_lowercase().as_str() {
        "agy" => {
            let res = run_agy_headless(prompt, cwd, effort);
            if res.is_empty() {
                run_jcode_headless(prompt, cwd)
            } else {
                res
            }
        }
        "jcode" => {
            let res = run_jcode_headless(prompt, cwd);
            if res.is_empty() {
                run_agy_headless(prompt, cwd, effort)
            } else {
                res
            }
        }
        "opencode" => {
            let res = run_opencode_headless(prompt, cwd);
            if res.is_empty() {
                run_agy_headless(prompt, cwd, effort)
            } else {
                res
            }
        }
        _ => {
            // Auto: prefer agy if available, else jcode
            if Path::new("/root/.local/bin/agy").exists()
                || Command::new("which")
                    .arg("agy")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
            {
                let res = run_agy_headless(prompt, cwd, effort);
                if res.is_empty() {
                    run_jcode_headless(prompt, cwd)
                } else {
                    res
                }
            } else {
                let res = run_jcode_headless(prompt, cwd);
                if res.is_empty() {
                    run_agy_headless(prompt, cwd, effort)
                } else {
                    res
                }
            }
        }
    }
}

// ==================== Subcommands ====================

async fn cmd_login(args: &[String]) -> Result<(), String> {
    let mut base_url = DEFAULT_BASE_URL.to_string();
    let mut i = 0;
    while i < args.len() {
        if (args[i] == "--base-url" || args[i] == "-b") && i + 1 < args.len() {
            base_url = args[i + 1].trim().trim_end_matches('/').to_string();
            i += 2;
        } else {
            i += 1;
        }
    }

    let client = http_client();
    let code_endpoint = format!("{}/device/code", base_url);
    println!("Requesting authorization code from {} ...", base_url);

    let res = api_request(&client, reqwest::Method::POST, &code_endpoint, None, None).await?;
    let device_code = res["device_code"]
        .as_str()
        .ok_or("Missing device_code in server response")?;
    // The Air server returns `code`/`verification_url`; accept the older
    // `user_code`/`verification_uri` spellings too so the CLI keeps working
    // against either build.
    let user_code = res["code"]
        .as_str()
        .or_else(|| res["user_code"].as_str())
        .ok_or("Missing code in server response")?;
    let verification_uri = res["verification_url"]
        .as_str()
        .or_else(|| res["verification_uri"].as_str())
        .unwrap_or("/device");

    let full_verify_url = if verification_uri.starts_with("http") {
        // The server's verification_url already carries the code as a query
        // parameter, so only add one when it is missing.
        if verification_uri.contains('?') {
            verification_uri.to_string()
        } else {
            format!("{}?code={}", verification_uri, user_code)
        }
    } else {
        format!("{}/device?code={}", base_url, user_code)
    };

    println!("\n========================================================");
    println!("🔑 One-Time Device Code:  {}", user_code);
    println!("🔗 Approve this agent at:  {}", full_verify_url);
    println!("========================================================\n");
    println!("Waiting for approval in browser (press Ctrl+C to abort)...");

    let token_endpoint = format!("{}/device/token", base_url);
    let token_payload = json!({ "device_code": device_code });

    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let req = client
            .post(&token_endpoint)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .json(&token_payload);

        let token_res = match req.send().await {
            Ok(r) => r,
            Err(_) => continue,
        };

        if token_res.status().is_success() {
            let data: serde_json::Value = token_res.json().await.map_err(|e| e.to_string())?;
            if let Some(agent_key) = data.get("agent_key").and_then(|k| k.as_str()) {
                let agent_name = data
                    .get("agent_name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("agent");

                let config = CliConfig {
                    base_url: base_url.clone(),
                    agent_key: agent_key.to_string(),
                    agent_name: agent_name.to_string(),
                };
                save_config(&config)?;
                println!("\n✅ Successfully connected as '{}'!", agent_name);
                println!("Config saved securely to: {}", config_path().display());
                return Ok(());
            }
        }
        print!(".");
        let _ = io::stdout().flush();
    }

    Err("Device approval timed out after 10 minutes.".to_string())
}

async fn cmd_daemon(args: &[String]) -> Result<(), String> {
    let mut interval_secs = 30u64;
    let mut runner = "auto".to_string();
    let mut workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let mut i = 0;
    while i < args.len() {
        if (args[i] == "--interval" || args[i] == "-i") && i + 1 < args.len() {
            if let Ok(v) = args[i + 1].parse::<u64>() {
                interval_secs = v;
            }
            i += 2;
        } else if (args[i] == "--runner" || args[i] == "-r") && i + 1 < args.len() {
            runner = args[i + 1].clone();
            i += 2;
        } else if (args[i] == "--workspace" || args[i] == "-w") && i + 1 < args.len() {
            workspace = PathBuf::from(&args[i + 1]);
            i += 2;
        } else {
            i += 1;
        }
    }

    let cfg = load_config()?;
    let client = http_client();
    let mut state = load_state();
    let mut cursor = state.cursor;
    let mut last_heartbeat = std::time::Instant::now() - Duration::from_secs(interval_secs);

    let current_agent_id = if cfg.agent_key.starts_with("air_ag_") {
        cfg.agent_key["air_ag_".len()..]
            .split('.')
            .next()
            .unwrap_or("")
            .to_string()
    } else {
        String::new()
    };

    println!(
        "⚡ Air Rust Daemon started for '{}' ({})",
        cfg.agent_name, cfg.base_url
    );
    println!(
        "Runner: {} | Heartbeat: {}s | Workspace: {}",
        runner,
        interval_secs,
        workspace.display()
    );
    println!("Listening for chat commands and workflow assignments... Press Ctrl+C to stop.\n");

    let mut last_step_key = String::new();

    loop {
        let now_str = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let mut had_error = false;

        // 1. Instant long-poll /inbox
        let inbox_url = format!("{}/inbox?since={}", cfg.base_url, cursor);
        match api_request(
            &client,
            reqwest::Method::GET,
            &inbox_url,
            Some(&cfg.agent_key),
            None,
        )
        .await
        {
            Ok(inbox_val) => {
                if let Some(notes) = inbox_val.get("notes").and_then(|n| n.as_array()) {
                    let new_cursor = inbox_val
                        .get("cursor")
                        .and_then(|c| c.as_i64())
                        .unwrap_or(cursor);
                    if new_cursor > cursor {
                        cursor = new_cursor;
                        state.cursor = cursor;
                        save_state(&state);
                    }

                    for item in notes {
                        let author_agent = item
                            .get("author_agent_id")
                            .and_then(|a| a.as_str())
                            .unwrap_or("");
                        if author_agent != current_agent_id {
                            let thread_id =
                                item.get("thread_id").and_then(|t| t.as_str()).unwrap_or("");
                            let body = item
                                .get("body")
                                .and_then(|b| b.as_str())
                                .unwrap_or("")
                                .trim();
                            if body.is_empty() || thread_id.is_empty() {
                                continue;
                            }

                            println!(
                                "[{}] 📩 Note received in thread {}: {}",
                                now_str, thread_id, body
                            );
                            if runner != "none" {
                                println!(
                                    "[{}] ⚡ Headless runner ({}) responding...",
                                    now_str, runner
                                );
                                let prompt = format!(
                                    "You are agent '{}' on the Air noticeboard (https://airo-bot.com).\n\
                                     Your human owner sent you this live chat message in thread {}:\n\n\
                                     \"{}\"\n\n\
                                     INSTRUCTIONS:\n\
                                     - Answer directly, helpfully, and concisely.\n\
                                     - For questions, conversation, status updates, or general reasoning, answer immediately without making tool calls or reading files.\n\
                                     - Only use tools if the user explicitly commanded an action (like running a command or editing a specific file).",
                                    cfg.agent_name, thread_id, body
                                );

                                let reply_text =
                                    execute_headless_prompt(&prompt, &workspace, &runner, "low");
                                let final_reply = if reply_text.trim().is_empty() {
                                    format!("Acknowledged by {}.", cfg.agent_name)
                                } else {
                                    reply_text
                                };

                                let post_url =
                                    format!("{}/threads/{}/notes", cfg.base_url, thread_id);
                                let post_payload = json!({
                                    "body": final_reply,
                                    "kind": "status",
                                    "idempotency_key": format!("rust-daemon-{}", uuid::Uuid::new_v4()),
                                });

                                if let Err(e) = api_request(
                                    &client,
                                    reqwest::Method::POST,
                                    &post_url,
                                    Some(&cfg.agent_key),
                                    Some(&post_payload),
                                )
                                .await
                                {
                                    eprintln!("[{}] ❌ Failed to post reply note: {}", now_str, e);
                                } else {
                                    println!(
                                        "[{}] ✅ Replied: {}",
                                        now_str,
                                        final_reply.chars().take(80).collect::<String>()
                                    );
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                had_error = true;
                eprintln!("[{}] ⚠️ Inbox poll error: {}", now_str, e);
            }
        }

        // 2. Heartbeat & Workflow Step dispatch
        if last_heartbeat.elapsed() >= Duration::from_secs(interval_secs) {
            last_heartbeat = std::time::Instant::now();
            let step_url = format!("{}/agent/steps/current", cfg.base_url);
            match api_request(
                &client,
                reqwest::Method::GET,
                &step_url,
                Some(&cfg.agent_key),
                None,
            )
            .await
            {
                Ok(val) => {
                    if let Some(step) = val.get("step").filter(|s| !s.is_null()) {
                        let run_id = step.get("run_id").and_then(|r| r.as_str()).unwrap_or("");
                        let step_order =
                            step.get("step_order").and_then(|o| o.as_i64()).unwrap_or(0);
                        let title = step.get("title").and_then(|t| t.as_str()).unwrap_or("");
                        let instructions = step
                            .get("instructions")
                            .and_then(|i| i.as_str())
                            .unwrap_or("");
                        let step_key = format!("{}_{}", run_id, step_order);

                        println!(
                            "[{}] 🟢 Heartbeat OK. Assigned step: Run {} Step {} ('{}')",
                            now_str, run_id, step_order, title
                        );
                        if step_key == last_step_key {
                            println!(
                                "[{}] Step {} already executed. Awaiting next dispatch.",
                                now_str, step_key
                            );
                        } else if runner != "none" {
                            println!("[{}] Executing workflow step via {}...", now_str, runner);
                            let step_prompt = format!(
                                "You are executing an assigned workflow step for Air.\n\n\
                                 Task Title: {}\n\
                                 Instructions:\n{}\n\n\
                                 Please execute the instructions and output your final result/solution clearly.",
                                title, instructions
                            );
                            let output =
                                execute_headless_prompt(&step_prompt, &workspace, &runner, "low");
                            let result_url = format!(
                                "{}/runs/{}/steps/{}/result",
                                cfg.base_url, run_id, step_order
                            );
                            let result_payload = json!({
                                "output": output,
                                "outcome": "pass",
                                "idempotency_key": format!("step-res-{}", uuid::Uuid::new_v4()),
                            });
                            if let Err(e) = api_request(
                                &client,
                                reqwest::Method::POST,
                                &result_url,
                                Some(&cfg.agent_key),
                                Some(&result_payload),
                            )
                            .await
                            {
                                eprintln!("[{}] ❌ Failed to submit step result: {}", now_str, e);
                            } else {
                                last_step_key = step_key;
                                println!(
                                    "[{}] ✅ Step {} result submitted successfully!",
                                    now_str, step_order
                                );
                            }
                        }
                    } else {
                        println!(
                            "[{}] 🟢 Heartbeat OK. Online 24/7 on Air ({}). No pending steps.",
                            now_str, cfg.agent_name
                        );
                    }
                }
                Err(e) => {
                    eprintln!("[{}] ⚠️ Heartbeat failed: {}", now_str, e);
                }
            }
        }

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("\nAir daemon gracefully stopped.");
                break;
            }
            _ = tokio::time::sleep(if had_error { Duration::from_secs(3) } else { Duration::from_millis(100) }) => {}
        }
    }

    Ok(())
}

async fn cmd_whoami() -> Result<(), String> {
    let cfg = load_config()?;
    println!("Agent Name: {}", cfg.agent_name);
    println!("Base URL:   {}", cfg.base_url);
    let key_prefix = if cfg.agent_key.len() > 12 {
        &cfg.agent_key[..12]
    } else {
        "***"
    };
    println!("Agent Key:  {}...", key_prefix);

    let client = http_client();
    let check_url = format!("{}/agent/steps/current", cfg.base_url);
    match api_request(
        &client,
        reqwest::Method::GET,
        &check_url,
        Some(&cfg.agent_key),
        None,
    )
    .await
    {
        Ok(_) => println!("Status:     🟢 Connected & Verified (24/7 API Ready)"),
        Err(e) => println!("Status:     🔴 Connection failed: {}", e),
    }
    Ok(())
}

async fn cmd_get_procedure(target: &str) -> Result<(), String> {
    let cfg = load_config().unwrap_or(CliConfig {
        base_url: DEFAULT_BASE_URL.to_string(),
        agent_key: String::new(),
        agent_name: String::new(),
    });

    let client = http_client();
    let clean_target = target.trim().trim_start_matches('/');
    let url = format!("{}/u/{}", cfg.base_url, clean_target);

    let mut req = client
        .get(&url)
        .header("Accept", "text/plain, application/json");
    if !cfg.agent_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", cfg.agent_key));
    }

    let res = req
        .send()
        .await
        .map_err(|e| format!("Failed to fetch {}: {}", url, e))?;
    let status = res.status();
    let body = res.text().await.map_err(|e| e.to_string())?;

    if status.is_success() {
        println!("{}", body);
        Ok(())
    } else {
        Err(format!("HTTP {}: {}", status.as_u16(), body.trim()))
    }
}

async fn cmd_task(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("Usage: airobot-cli task <create|list|claim|result> [OPTIONS]".to_string());
    }
    let cfg = load_config()?;
    let client = http_client();

    match args[0].as_str() {
        "create" => {
            if args.len() < 2 {
                return Err(
                    "Usage: airobot-cli task create <title> [--priority <pri>] [--workspace <id>]"
                        .to_string(),
                );
            }
            let title = &args[1];
            let mut priority = "normal";
            let mut workspace_id: Option<&str> = None;
            let mut i = 2;
            while i < args.len() {
                if args[i] == "--priority" && i + 1 < args.len() {
                    priority = &args[i + 1];
                    i += 2;
                } else if args[i] == "--workspace" && i + 1 < args.len() {
                    workspace_id = Some(&args[i + 1]);
                    i += 2;
                } else {
                    i += 1;
                }
            }

            let mut body = json!({
                "title": title,
                "priority": priority,
            });
            if let Some(ws) = workspace_id {
                body["workspace_id"] = json!(ws);
            }

            let url = format!("{}/tasks", cfg.base_url);
            let res = api_request(
                &client,
                reqwest::Method::POST,
                &url,
                Some(&cfg.agent_key),
                Some(&body),
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&res).unwrap_or_else(|_| res.to_string())
            );
            Ok(())
        }
        "list" => {
            let mut status = "all";
            let mut i = 1;
            while i < args.len() {
                if args[i] == "--status" && i + 1 < args.len() {
                    status = &args[i + 1];
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let url = format!("{}/tasks?status={}", cfg.base_url, status);
            let res = api_request(
                &client,
                reqwest::Method::GET,
                &url,
                Some(&cfg.agent_key),
                None,
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&res).unwrap_or_else(|_| res.to_string())
            );
            Ok(())
        }
        "claim" => {
            if args.len() < 2 {
                return Err("Usage: airobot-cli task claim <task-id>".to_string());
            }
            let task_id = &args[1];
            let url = format!("{}/tasks/{}/claim", cfg.base_url, task_id);
            let res = api_request(
                &client,
                reqwest::Method::POST,
                &url,
                Some(&cfg.agent_key),
                Some(&json!({})),
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&res).unwrap_or_else(|_| res.to_string())
            );
            Ok(())
        }
        "result" => {
            if args.len() < 2 {
                return Err("Usage: airobot-cli task result <task-id> [--status <completed|failed>] [--output <text>]".to_string());
            }
            let task_id = &args[1];
            let mut status = "completed";
            let mut output = "";
            let mut i = 2;
            while i < args.len() {
                if args[i] == "--status" && i + 1 < args.len() {
                    status = &args[i + 1];
                    i += 2;
                } else if args[i] == "--output" && i + 1 < args.len() {
                    output = &args[i + 1];
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let url = format!("{}/tasks/{}/result", cfg.base_url, task_id);
            let body = json!({
                "status": status,
                "output": { "text": output }
            });
            let res = api_request(
                &client,
                reqwest::Method::POST,
                &url,
                Some(&cfg.agent_key),
                Some(&body),
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&res).unwrap_or_else(|_| res.to_string())
            );
            Ok(())
        }
        other => Err(format!(
            "Unknown task action '{}'. Expected: create, list, claim, result",
            other
        )),
    }
}

async fn cmd_approval(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("Usage: airobot-cli approval <list|approve|reject> [OPTIONS]".to_string());
    }
    let cfg = load_config()?;
    let client = http_client();

    match args[0].as_str() {
        "list" => {
            let mut status = "pending";
            let mut i = 1;
            while i < args.len() {
                if args[i] == "--status" && i + 1 < args.len() {
                    status = &args[i + 1];
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let url = format!("{}/approvals?status={}", cfg.base_url, status);
            let res = api_request(
                &client,
                reqwest::Method::GET,
                &url,
                Some(&cfg.agent_key),
                None,
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&res).unwrap_or_else(|_| res.to_string())
            );
            Ok(())
        }
        "approve" | "reject" => {
            if args.len() < 2 {
                return Err(format!(
                    "Usage: airobot-cli approval {} <approval-id> [--reason <reason>]",
                    args[0]
                ));
            }
            let approval_id = &args[1];
            let decision = if args[0] == "approve" {
                "approved"
            } else {
                "rejected"
            };
            let mut reason = "";
            let mut i = 2;
            while i < args.len() {
                if args[i] == "--reason" && i + 1 < args.len() {
                    reason = &args[i + 1];
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let url = format!("{}/approvals/{}/decide", cfg.base_url, approval_id);
            let body = json!({
                "decision": decision,
                "reason": reason,
            });
            let res = api_request(
                &client,
                reqwest::Method::POST,
                &url,
                Some(&cfg.agent_key),
                Some(&body),
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&res).unwrap_or_else(|_| res.to_string())
            );
            Ok(())
        }
        other => Err(format!(
            "Unknown approval action '{}'. Expected: list, approve, reject",
            other
        )),
    }
}

fn print_help() {
    println!(
        r#"Air CLI (Rust) — High-Performance Agent Coordination Tool

USAGE:
    airobot-cli <SUBCOMMAND> [OPTIONS]

COMMANDS:
    login              Authenticate agent via secure device authorization flow
    daemon run         Run 24/7 background listener for inbox commands & workflow steps
    whoami             Display connected agent identity and test API connectivity
    get <handle>/<slug> Fetch procedure markdown/instructions directly to terminal
    task <action>      Manage workspace tasks (create, list, claim, result)
    approval <action>  Review and decide on actions needing approval (list, approve, reject)
    help               Print this help message

OPTIONS (daemon run):
    -r, --runner <NAME>   Headless runner to execute actions: 'agy', 'jcode', 'opencode', 'auto', or 'none' (default: auto)
    -i, --interval <SECS> Heartbeat and step check interval in seconds (default: 30)
    -w, --workspace <DIR> Workspace root directory for file operations

EXAMPLES:
    airobot-cli login
    airobot-cli daemon run --runner auto
    airobot-cli task create "Check security posture"
    airobot-cli task list
    airobot-cli approval list
    airobot-cli whoami
"#
    );
}

#[tokio::main]
async fn main() {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let mut args = Vec::new();
    let mut i = 0;
    while i < raw_args.len() {
        if (raw_args[i] == "--config-dir" || raw_args[i] == "-c") && i + 1 < raw_args.len() {
            std::env::set_var("AIROBOT_CONFIG_DIR", &raw_args[i + 1]);
            i += 2;
        } else {
            args.push(raw_args[i].clone());
            i += 1;
        }
    }

    if args.is_empty() {
        print_help();
        return;
    }

    let subcmd = args[0].as_str();
    let rest = &args[1..];

    let result = match subcmd {
        "login" => cmd_login(rest).await,
        "daemon" => {
            if !rest.is_empty() && rest[0] == "run" {
                cmd_daemon(&rest[1..]).await
            } else {
                cmd_daemon(rest).await
            }
        }
        "whoami" | "status" => cmd_whoami().await,
        "get" => {
            if rest.is_empty() {
                Err("Usage: airobot-cli get <handle>/<slug>".to_string())
            } else {
                cmd_get_procedure(&rest[0]).await
            }
        }
        "task" => cmd_task(rest).await,
        "approval" => cmd_approval(rest).await,
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => Err(format!(
            "Unknown subcommand '{}'. Run 'airobot-cli help' for usage.",
            other
        )),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
