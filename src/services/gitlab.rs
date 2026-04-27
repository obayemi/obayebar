//! Pulls the authenticated user's GitLab Todo inbox.
//!
//! Authentication uses a Personal Access Token (PAT) with the `read_api`
//! scope. The token is sourced, in order of precedence, from:
//!   1. `OBAYEBAR_GITLAB_TOKEN` environment variable
//!   2. The Secret Service keyring (`org.freedesktop.secrets`) under
//!      attributes `service=obayebar`, `key=gitlab_token` — used when a
//!      keyring daemon is running and its default collection is unlocked.
//!   3. `$XDG_CONFIG_HOME/obayebar/gitlab_token` (or `~/.config/...`)
//!
//! Saving prefers the keyring; if the service isn't running (or its default
//! collection is locked), the token falls back to the plain file. "Forget
//! token" clears both.
//!
//! Host resolution lives in `crate::config`.

use std::path::PathBuf;
use std::time::Duration;

use crate::services::dbus_util::PanelSignal;
use futures_util::Stream;
use serde::Deserialize;

/// Browser destination for the "Show all" button.
pub const TODO_PAGE_PATH: &str = "/dashboard/todos";

const POLL_INTERVAL_OPEN: Duration = Duration::from_secs(30);
const POLL_INTERVAL_CLOSED: Duration = Duration::from_mins(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const PER_PAGE: u32 = 100;

static PANEL: PanelSignal = PanelSignal::new();

/// Toggle from the UI when the gitlab panel opens/closes.
pub fn set_panel_open(open: bool) {
    PANEL.set(open);
}

/// Public read view of the service state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitlabInfo {
    pub auth: AuthState,
    /// Up to ~visible+overflow todos in display order (newest first).
    pub todos: Vec<TodoItem>,
    /// Total open todos as reported by the API (may exceed `todos.len()`).
    pub total: usize,
    /// Last error message, if the most recent fetch failed.
    pub error: Option<String>,
}

/// Resolved host for "Show all" / token-creation URLs. Static after `main`.
pub fn host() -> &'static str {
    crate::config::resolved().gitlab_host()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AuthState {
    /// No token configured.
    #[default]
    Missing,
    /// Token configured and at least one fetch succeeded.
    Authenticated,
    /// Token configured but the API rejected it (401/403).
    Invalid,
}

/// Trimmed-down view of a GitLab todo for rendering in the popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub id: u64,
    pub title: String,
    pub project: String,
    pub action: String,
    pub target_type: String,
    pub url: String,
}

#[derive(Deserialize)]
struct ApiTodo {
    id: u64,
    #[serde(default)]
    action_name: Option<String>,
    #[serde(default)]
    target_type: Option<String>,
    #[serde(default)]
    target_url: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    target: Option<ApiTarget>,
    #[serde(default)]
    project: Option<ApiProject>,
}

#[derive(Deserialize)]
struct ApiTarget {
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
struct ApiProject {
    #[serde(default)]
    name_with_namespace: Option<String>,
    #[serde(default)]
    path_with_namespace: Option<String>,
}

/// Resolved GitLab connection settings. The host is process-static, see
/// [`host`].
#[derive(Debug, Clone)]
struct Settings {
    token: Option<String>,
}

/// Path the user can drop a token file at; surfaced in the popup.
pub fn token_file_path() -> Option<PathBuf> {
    obayebar::xdg::config_dir().map(|d| d.join("gitlab_token"))
}

/// Secret Service item attributes that uniquely identify our token.
fn keyring_attrs() -> std::collections::HashMap<&'static str, &'static str> {
    std::collections::HashMap::from([("service", "obayebar"), ("key", "gitlab_token")])
}

const KEYRING_LABEL: &str = "obayebar GitLab access token";

/// Read the token from the Secret Service keyring's default collection.
/// Only considers `unlocked` items so we never block on an unlock prompt.
async fn keyring_load_token() -> Option<String> {
    let ss = secret_service::SecretService::connect(secret_service::EncryptionType::Plain)
        .await
        .ok()?;
    let items = ss.search_items(keyring_attrs()).await.ok()?;
    let item = items.unlocked.into_iter().next()?;
    let secret = item.get_secret().await.ok()?;
    String::from_utf8(secret).ok()
}

/// Save the token into the Secret Service default collection. Returns
/// `Err` (with a debug-level message) if the service isn't running or the
/// collection is locked — callers should fall back to the file.
async fn keyring_save_token(token: &str) -> Result<(), String> {
    let ss = secret_service::SecretService::connect(secret_service::EncryptionType::Plain)
        .await
        .map_err(|e| e.to_string())?;
    let collection = ss
        .get_default_collection()
        .await
        .map_err(|e| e.to_string())?;
    if collection.is_locked().await.map_err(|e| e.to_string())? {
        return Err("default collection is locked".to_string());
    }
    collection
        .create_item(
            KEYRING_LABEL,
            keyring_attrs(),
            token.as_bytes(),
            true,
            "text/plain",
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete every matching item from both unlocked and locked sets. Locked
/// items can be deleted without unlocking — only their secret value is
/// gated behind unlock prompts.
async fn keyring_clear_token() -> Result<(), String> {
    let ss = secret_service::SecretService::connect(secret_service::EncryptionType::Plain)
        .await
        .map_err(|e| e.to_string())?;
    let items = ss
        .search_items(keyring_attrs())
        .await
        .map_err(|e| e.to_string())?;
    for item in items.unlocked.into_iter().chain(items.locked) {
        item.delete().await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Resolve token from env, then keyring, then file. Trims whitespace.
async fn load_token() -> Option<String> {
    if let Ok(t) = std::env::var("OBAYEBAR_GITLAB_TOKEN") {
        let trimmed = t.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    if let Some(t) = keyring_load_token().await {
        let trimmed = t.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let path = token_file_path()?;
    let content = tokio::fs::read_to_string(&path).await.ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

async fn load_settings() -> Settings {
    Settings {
        token: load_token().await,
    }
}

fn build_item(api: ApiTodo) -> TodoItem {
    let title = api
        .target
        .as_ref()
        .and_then(|t| t.title.clone())
        .or(api.body)
        .unwrap_or_default();
    let project = api
        .project
        .and_then(|p| p.name_with_namespace.or(p.path_with_namespace))
        .unwrap_or_default();
    TodoItem {
        id: api.id,
        title,
        project,
        action: api.action_name.unwrap_or_default(),
        target_type: api.target_type.unwrap_or_default(),
        url: api.target_url.unwrap_or_default(),
    }
}

async fn fetch_todos(
    client: &reqwest::Client,
    settings: &Settings,
) -> Result<(Vec<TodoItem>, usize), FetchError> {
    let token = settings.token.as_deref().ok_or(FetchError::Missing)?;
    let url = format!("{}/api/v4/todos", host());
    let resp = client
        .get(&url)
        .query(&[("state", "pending"), ("per_page", &PER_PAGE.to_string())])
        .header("PRIVATE-TOKEN", token)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(FetchError::Invalid);
    }
    if !status.is_success() {
        return Err(FetchError::Network(format!("HTTP {status}")));
    }

    let total_header = resp
        .headers()
        .get("x-total")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok());
    let raw: Vec<ApiTodo> = resp
        .json()
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;
    let total = total_header.unwrap_or(raw.len());
    let items: Vec<TodoItem> = raw.into_iter().map(build_item).collect();
    Ok((items, total))
}

#[derive(Debug)]
enum FetchError {
    Missing,
    Invalid,
    Network(String),
}

/// Open the URL in the user's preferred browser, then ask Hyprland to focus
/// the browser's window so the user lands on the new tab instead of having
/// the bar's workspace stay active. Best-effort, fire-and-forget.
pub fn open_in_browser(url: String) {
    tokio::spawn(async move {
        if let Err(e) = tokio::process::Command::new("xdg-open").arg(&url).spawn() {
            log::warn!("gitlab: xdg-open failed: {e}");
            return;
        }
        // Resolve the browser's window class up front; this is independent
        // of how long the browser takes to handle the URL.
        let Some(class) = default_browser_class().await else {
            log::debug!("gitlab: no default browser class detected, skipping focus");
            return;
        };
        // Give the browser a moment to either raise its window (already
        // running) or spawn (cold start). 400ms covers most fast paths;
        // cold-start cases lose focus and the user clicks manually.
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        crate::services::hyprland::focus_window_class(&class);
    });
}

/// Best-effort lookup of the window class for the default `https` handler.
/// Reads `xdg-mime` to find the .desktop file, then prefers `StartupWMClass`
/// over the desktop file basename.
async fn default_browser_class() -> Option<String> {
    let out = tokio::process::Command::new("xdg-mime")
        .args(["query", "default", "x-scheme-handler/https"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let desktop_id = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if desktop_id.is_empty() {
        return None;
    }
    if let Some(class) = read_startup_wm_class(&desktop_id).await {
        return Some(class);
    }
    desktop_id
        .strip_suffix(".desktop")
        .map(std::string::ToString::to_string)
}

async fn read_startup_wm_class(desktop_id: &str) -> Option<String> {
    for dir in xdg_data_dirs() {
        let path = format!("{dir}/applications/{desktop_id}");
        let Ok(content) = tokio::fs::read_to_string(&path).await else {
            continue;
        };
        let mut in_entry = false;
        for line in content.lines() {
            let line = line.trim();
            if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                in_entry = section == "Desktop Entry";
                continue;
            }
            if !in_entry {
                continue;
            }
            if let Some(value) = line.strip_prefix("StartupWMClass=") {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

fn xdg_data_dirs() -> Vec<String> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(format!("{home}/.local/share"));
    }
    if let Ok(xdg_data) = std::env::var("XDG_DATA_DIRS") {
        for dir in xdg_data.split(':').filter(|s| !s.is_empty()) {
            dirs.push(dir.to_string());
        }
    } else {
        dirs.push("/usr/local/share".to_string());
        dirs.push("/usr/share".to_string());
    }
    dirs
}

/// Open the user's editor on the token file path, creating the directory first.
pub fn open_token_file() {
    tokio::spawn(async {
        let Some(path) = token_file_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if !path.exists() {
            let _ = tokio::fs::write(&path, b"").await;
        }
        if let Err(e) = tokio::process::Command::new("xdg-open").arg(&path).spawn() {
            log::warn!("gitlab: xdg-open token file failed: {e}");
        }
    });
}

/// Notify the polling loop to refresh on the next tick (e.g. after the user
/// updated the token file).
pub fn request_refresh() {
    REFRESH.notify_waiters();
}

/// Read the system clipboard via `wl-paste`/`xclip`. Used to fill the token
/// input on demand without keeping the iced clipboard worker thread alive.
/// Returns `Ok(text)` (possibly empty after trimming) or an error message
/// suitable for the popup's error line.
pub async fn read_clipboard() -> Result<String, String> {
    let candidates: &[(&str, &[&str])] = &[
        ("wl-paste", &["--no-newline"]),
        ("xclip", &["-selection", "clipboard", "-o"]),
    ];
    let mut last_err = String::new();
    for (cmd, args) in candidates {
        match tokio::process::Command::new(cmd).args(*args).output().await {
            Ok(out) if out.status.success() => {
                return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
            }
            Ok(out) => {
                last_err = format!(
                    "{cmd} exited with status {} ({})",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim(),
                );
            }
            Err(e) => {
                last_err = format!("{cmd}: {e}");
            }
        }
    }
    Err(if last_err.is_empty() {
        "No clipboard tool available (install wl-clipboard or xclip)".to_string()
    } else {
        last_err
    })
}

/// Save `token`, preferring the Secret Service keyring. Falls back to the
/// `0600`-permissioned token file when the keyring isn't reachable.
pub async fn save_token(token: String) -> Result<(), String> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err("Token is empty".to_string());
    }
    match keyring_save_token(trimmed).await {
        Ok(()) => return Ok(()),
        Err(e) => log::info!("gitlab: keyring unavailable ({e}), falling back to token file"),
    }
    write_token_file(trimmed)
        .await
        .map_err(|e| format!("Could not write token file: {e}"))
}

/// Forget the token from both the keyring and the on-disk file. Best-effort
/// for the keyring (a missing daemon is not an error); strict for the file
/// only if it exists.
pub async fn forget_token() -> Result<(), String> {
    if let Err(e) = keyring_clear_token().await {
        log::debug!("gitlab: keyring clear skipped ({e})");
    }
    let Some(path) = token_file_path() else {
        return Ok(());
    };
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Could not remove token file: {e}")),
    }
}

/// Open `path` truncating, creating it with `0600` perms in a single `open()`
/// call on Unix. The bar is Wayland-only in practice but the non-Unix branch
/// keeps `cargo check` green elsewhere.
async fn open_token_file_for_write(path: &std::path::Path) -> std::io::Result<tokio::fs::File> {
    let mut opts = tokio::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    opts.mode(0o600);
    opts.open(path).await
}

async fn write_token_file(token: &str) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let Some(path) = token_file_path() else {
        return Err(std::io::Error::other("no config dir"));
    };
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = open_token_file_for_write(&path).await?;
    file.write_all(token.as_bytes()).await?;
    file.write_all(b"\n").await?;
    Ok(())
}

static REFRESH: std::sync::LazyLock<tokio::sync::Notify> =
    std::sync::LazyLock::new(tokio::sync::Notify::new);

pub fn stream() -> impl Stream<Item = GitlabInfo> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move { run_loop(tx).await });
    tokio_stream::wrappers::UnboundedReceiverStream::new(rx)
}

async fn run_loop(tx: tokio::sync::mpsc::UnboundedSender<GitlabInfo>) {
    let Ok(client) = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("obayebar/", env!("CARGO_PKG_VERSION")))
        .build()
    else {
        log::warn!("gitlab: failed to build HTTP client");
        return;
    };

    let mut settings = load_settings().await;
    let mut last = GitlabInfo {
        auth: if settings.token.is_some() {
            AuthState::Authenticated
        } else {
            AuthState::Missing
        },
        ..GitlabInfo::default()
    };
    if tx.send(last.clone()).is_err() {
        return;
    }

    loop {
        // Re-read token each tick so external edits (token file, keyring
        // entries set elsewhere) take effect without a restart.
        settings = load_settings().await;
        let info = match fetch_todos(&client, &settings).await {
            Ok((todos, total)) => GitlabInfo {
                auth: AuthState::Authenticated,
                todos,
                total,
                error: None,
            },
            Err(FetchError::Missing) => GitlabInfo {
                auth: AuthState::Missing,
                todos: Vec::new(),
                total: 0,
                error: None,
            },
            Err(FetchError::Invalid) => GitlabInfo {
                auth: AuthState::Invalid,
                todos: Vec::new(),
                total: 0,
                error: Some("Token rejected by GitLab".to_string()),
            },
            // Transient network failure: keep the previous payload, just
            // attach the new error string. Avoid cloning the cached todos
            // by mutating `last` in place when nothing else changed.
            Err(FetchError::Network(msg)) => {
                if last.error.as_deref() != Some(msg.as_str()) {
                    last.error = Some(msg);
                    if tx.send(last.clone()).is_err() {
                        return;
                    }
                }
                wait_next_tick().await;
                continue;
            }
        };

        if info != last {
            last = info.clone();
            if tx.send(info).is_err() {
                return;
            }
        }

        wait_next_tick().await;
    }
}

async fn wait_next_tick() {
    let interval = if PANEL.is_open() {
        POLL_INTERVAL_OPEN
    } else {
        POLL_INTERVAL_CLOSED
    };
    tokio::select! {
        () = tokio::time::sleep(interval) => {}
        () = PANEL.changed() => {}
        () = REFRESH.notified() => {}
    }
}

/// Format a todo `action_name` field into a user-friendly verb.
pub fn format_action(action: &str) -> &str {
    match action {
        "assigned" => "assigned",
        "review_requested" => "review requested",
        "mentioned" | "directly_addressed" => "mentioned",
        "build_failed" => "build failed",
        "marked" => "marked",
        "approval_required" => "approval required",
        "unmergeable" => "unmergeable",
        "merge_train_removed" => "merge train removed",
        other => other,
    }
}

/// Format a todo `target_type` field into a short display label.
pub fn format_target_type(target_type: &str) -> &str {
    match target_type {
        "Issue" => "issue",
        "MergeRequest" => "merge request",
        "Commit" => "commit",
        "DesignManagement::Design" => "design",
        "AlertManagement::Alert" => "alert",
        "Epic" => "epic",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_item_prefers_target_title_over_body() {
        let api = ApiTodo {
            id: 1,
            action_name: Some("assigned".into()),
            target_type: Some("Issue".into()),
            target_url: Some("https://example.com/i/1".into()),
            body: Some("body fallback".into()),
            target: Some(ApiTarget {
                title: Some("Fix bug".into()),
            }),
            project: Some(ApiProject {
                name_with_namespace: Some("Group / Repo".into()),
                path_with_namespace: None,
            }),
        };
        let item = build_item(api);
        assert_eq!(item.title, "Fix bug");
        assert_eq!(item.project, "Group / Repo");
        assert_eq!(item.url, "https://example.com/i/1");
    }

    #[test]
    fn build_item_falls_back_to_body_then_path() {
        let api = ApiTodo {
            id: 2,
            action_name: None,
            target_type: None,
            target_url: None,
            body: Some("only body".into()),
            target: None,
            project: Some(ApiProject {
                name_with_namespace: None,
                path_with_namespace: Some("group/repo".into()),
            }),
        };
        let item = build_item(api);
        assert_eq!(item.title, "only body");
        assert_eq!(item.project, "group/repo");
    }
}
