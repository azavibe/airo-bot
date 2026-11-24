//! "Check for updates" (issue #54): compares this build's version with the
//! newest published Airobot release on GitHub and picks the installer for
//! the computer this server runs on. Meant for the desktop app, whose local
//! server runs on the person's own machine, so the operating system and
//! chip here are theirs.
//!
//! The answer is kept in memory for an hour per feed, so the app can ask
//! often without calling GitHub each time; `?refresh=1` (the "Check now"
//! button) asks again, at most once a minute.
//!
//! TODO(air): installing in place (download, verify, restart) needs the
//! Tauri updater and a signing key in the release workflow's secrets. Until
//! then the button downloads the right installer, which the person opens.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::error::AppError;
use crate::state::AppState;

/// Where published releases are listed (GitHub's newest non-draft,
/// non-prerelease release). `UPDATE_FEED_URL` replaces it; `UPDATE_CHECK=off`
/// turns checks off.
pub const DEFAULT_FEED: &str = "https://api.github.com/repos/azavibe/airo-bot/releases/latest";

/// The version this build carries (the release workflow stamps it).
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const KEEP_FOR: Duration = Duration::from_secs(60 * 60);
const REFRESH_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UpdateInfo {
    pub current: String,
    /// The newest published version, or null when it couldn't be read.
    pub latest: Option<String>,
    pub available: bool,
    /// The release page, with all installers and the notes.
    pub release_url: Option<String>,
    /// The installer for this computer, when one matches.
    pub download_url: Option<String>,
    pub published_at: Option<String>,
    /// Checks are switched off on this server.
    pub disabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct CheckQuery {
    pub refresh: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: Option<String>,
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

type Remembered = HashMap<String, (Instant, UpdateInfo)>;

fn remembered() -> &'static Mutex<Remembered> {
    static CACHE: OnceLock<Mutex<Remembered>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Numeric parts of `1.2.3` or `v1.2.3-beta.1`, and whether it is a
/// pre-release (which sorts before the release of the same numbers).
fn parse_version(v: &str) -> Option<(Vec<u64>, bool)> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((core, _)) => (core, true),
        None => (v, false),
    };
    let parts = core
        .split('.')
        .map(|p| p.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    (parts.len() == 3).then_some((parts, pre))
}

/// Whether `latest` is a newer version than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some((l, l_pre)), Some((c, c_pre))) => l > c || (l == c && c_pre && !l_pre),
        _ => false,
    }
}

/// The installer for an operating system and chip, by the names the release
/// workflow gives them (`Airobot_<v>_aarch64.dmg`, `_x64.dmg`,
/// `_x64-setup.exe`, `_x64_en-US.msi`).
fn installer_for<'a>(assets: &'a [Asset], os: &str, arch: &str) -> Option<&'a Asset> {
    let ends = |suffix: &str| assets.iter().find(|a| a.name.ends_with(suffix));
    match (os, arch) {
        ("macos", "aarch64") => ends("_aarch64.dmg"),
        ("macos", _) => ends("_x64.dmg"),
        ("windows", _) => ends("-setup.exe").or_else(|| ends(".msi")),
        _ => None,
    }
}

fn info_from(release: &Release, os: &str, arch: &str) -> UpdateInfo {
    let latest = release.tag_name.trim().trim_start_matches('v').to_string();
    let usable = !release.draft && !release.prerelease && parse_version(&latest).is_some();
    let https = |u: &str| u.starts_with("https://");
    UpdateInfo {
        current: CURRENT_VERSION.to_string(),
        available: usable && is_newer(&latest, CURRENT_VERSION),
        release_url: release.html_url.clone().filter(|u| https(u)),
        download_url: installer_for(&release.assets, os, arch)
            .map(|a| a.browser_download_url.clone())
            .filter(|u| https(u)),
        published_at: release.published_at.clone(),
        latest: usable.then_some(latest),
        disabled: false,
    }
}

async fn fetch(feed: &str) -> Result<Release, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent(concat!("airobot-update-check/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let res = client
        .get(feed)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("feed answered {}", res.status()));
    }
    res.json::<Release>().await.map_err(|e| e.to_string())
}

/// GET /api/app/update — is there a newer Airobot, and where is it?
pub async fn check_update(
    State(state): State<AppState>,
    _caller: Caller,
    Query(q): Query<CheckQuery>,
) -> Result<Json<UpdateInfo>, AppError> {
    let Some(feed) = state.config.update_feed.clone() else {
        return Ok(Json(UpdateInfo {
            current: CURRENT_VERSION.to_string(),
            latest: None,
            available: false,
            release_url: None,
            download_url: None,
            published_at: None,
            disabled: true,
        }));
    };
    let refresh = q
        .refresh
        .as_deref()
        .is_some_and(|v| v == "1" || v == "true");
    if let Ok(cache) = remembered().lock() {
        if let Some((at, info)) = cache.get(&feed) {
            let fresh = if refresh { REFRESH_AFTER } else { KEEP_FOR };
            if at.elapsed() < fresh {
                return Ok(Json(info.clone()));
            }
        }
    }
    match fetch(&feed).await {
        Ok(release) => {
            let info = info_from(&release, std::env::consts::OS, std::env::consts::ARCH);
            if let Ok(mut cache) = remembered().lock() {
                cache.insert(feed, (Instant::now(), info.clone()));
            }
            Ok(Json(info))
        }
        Err(error) => {
            tracing::warn!(%error, "update check failed");
            Err(AppError::ServiceUnavailable(
                "Couldn't reach GitHub to check for updates. Try again later.".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.to_string(),
            browser_download_url: format!("https://example.com/{name}"),
        }
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(is_newer("0.7.2", "0.7.1"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "1.0.0-beta.1"));
        assert!(!is_newer("0.7.1", "0.7.1"));
        assert!(!is_newer("0.7.0", "0.7.1"));
        assert!(!is_newer("1.0.0-beta.1", "1.0.0"));
        assert!(!is_newer("latest", "0.7.1"));
    }

    #[test]
    fn the_installer_matches_the_computer() {
        let assets = vec![
            asset("Airobot_0.7.2_aarch64.dmg"),
            asset("Airobot_0.7.2_x64.dmg"),
            asset("Airobot_0.7.2_x64_en-US.msi"),
            asset("Airobot_0.7.2_x64-setup.exe"),
        ];
        let name = |os, arch| installer_for(&assets, os, arch).map(|a| a.name.as_str());
        assert_eq!(name("macos", "aarch64"), Some("Airobot_0.7.2_aarch64.dmg"));
        assert_eq!(name("macos", "x86_64"), Some("Airobot_0.7.2_x64.dmg"));
        assert_eq!(
            name("windows", "x86_64"),
            Some("Airobot_0.7.2_x64-setup.exe")
        );
        assert_eq!(name("linux", "x86_64"), None);
    }

    #[test]
    fn drafts_and_prereleases_are_not_offered() {
        let release = |draft, prerelease| Release {
            tag_name: "v99.0.0".into(),
            html_url: Some("https://example.com/r".into()),
            published_at: None,
            draft,
            prerelease,
            assets: vec![],
        };
        assert!(info_from(&release(false, false), "linux", "x86_64").available);
        assert!(!info_from(&release(true, false), "linux", "x86_64").available);
        assert!(!info_from(&release(false, true), "linux", "x86_64").available);
    }
}
