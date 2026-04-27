//! Pulls the authenticated user's GitLab Todo inbox.
//!
//! Authentication uses a Personal Access Token (PAT) with the `read_api`
//! scope. The token is sourced, in order of precedence, from:
//!   1. `OBAYEBAR_GITLAB_TOKEN` environment variable
//!   2. `$XDG_CONFIG_HOME/obayebar/gitlab_token` (or `~/.config/...`)
//!
//! The host can be overridden via `OBAYEBAR_GITLAB_URL` (default
//! `https://gitlab.com`) for self-hosted instances.

use std::path::PathBuf;
use std::time::Duration;

use crate::services::dbus_util::PanelSignal;
use futures_util::Stream;
use serde::Deserialize;

/// Browser destination for the "Show all" button.
pub const TODO_PAGE_PATH: &str = "/dashboard/todos";

const DEFAULT_HOST: &str = "https://gitlab.com";
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
    /// Resolved host (e.g. `https://gitlab.com`), useful for the "Show all" link.
    pub host: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
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

/// Resolved GitLab connection settings.
#[derive(Debug, Clone)]
struct Settings {
    host: String,
    token: Option<String>,
}

fn config_dir() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("obayebar"));
        }
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config").join("obayebar"))
}

/// Path the user can drop a token file at; surfaced in the popup.
pub fn token_file_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("gitlab_token"))
}

/// Resolve token from env or the token file. Trims whitespace.
fn load_token() -> Option<String> {
    if let Ok(t) = std::env::var("OBAYEBAR_GITLAB_TOKEN") {
        let trimmed = t.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let path = token_file_path()?;
    let content = std::fs::read_to_string(&path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn load_settings() -> Settings {
    let host = std::env::var("OBAYEBAR_GITLAB_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map_or_else(
            || DEFAULT_HOST.to_string(),
            |s| s.trim_end_matches('/').to_string(),
        );
    Settings {
        host,
        token: load_token(),
    }
}

fn parse_total(headers: &reqwest::header::HeaderMap, fallback: usize) -> usize {
    headers
        .get("x-total")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(fallback)
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
    let url = format!("{}/api/v4/todos", settings.host);
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

    let headers = resp.headers().clone();
    let raw: Vec<ApiTodo> = resp
        .json()
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;
    let len = raw.len();
    let items: Vec<TodoItem> = raw.into_iter().map(build_item).collect();
    let total = parse_total(&headers, len);
    Ok((items, total))
}

#[derive(Debug)]
enum FetchError {
    Missing,
    Invalid,
    Network(String),
}

/// Open the URL in the user's preferred browser. Best-effort, fire-and-forget.
pub fn open_in_browser(url: String) {
    tokio::spawn(async move {
        if let Err(e) = tokio::process::Command::new("xdg-open").arg(&url).spawn() {
            log::warn!("gitlab: xdg-open failed: {e}");
        }
    });
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
    REFRESH.notify();
}

/// Read the Wayland clipboard, save the result as the GitLab token, and ask
/// the polling loop to refresh. Best-effort; logs failures and surfaces them
/// through the next emitted `GitlabInfo`.
pub fn paste_token_from_clipboard() {
    tokio::spawn(async move {
        let token = match read_clipboard().await {
            Ok(t) => t,
            Err(e) => {
                CLIPBOARD_ERROR.store(Some(e));
                REFRESH.notify();
                return;
            }
        };
        let trimmed = token.trim();
        if trimmed.is_empty() {
            CLIPBOARD_ERROR.store(Some("Clipboard is empty".to_string()));
            REFRESH.notify();
            return;
        }
        if let Err(e) = write_token_file(trimmed).await {
            CLIPBOARD_ERROR.store(Some(format!("Could not write token file: {e}")));
            REFRESH.notify();
            return;
        }
        CLIPBOARD_ERROR.clear();
        REFRESH.notify();
    });
}

/// Last clipboard-paste error, surfaced into the next `GitlabInfo` so the
/// popup can display it without an out-of-band channel.
static CLIPBOARD_ERROR: ClipboardError = ClipboardError::new();

#[derive(Debug)]
struct ClipboardError {
    inner: std::sync::OnceLock<std::sync::Mutex<Option<String>>>,
}

impl ClipboardError {
    const fn new() -> Self {
        Self {
            inner: std::sync::OnceLock::new(),
        }
    }
    fn cell(&self) -> &std::sync::Mutex<Option<String>> {
        self.inner.get_or_init(|| std::sync::Mutex::new(None))
    }
    fn store(&self, msg: Option<String>) {
        if let Ok(mut guard) = self.cell().lock() {
            *guard = msg;
        }
    }
    fn clear(&self) {
        self.store(None);
    }
    fn take(&self) -> Option<String> {
        self.cell().lock().ok().and_then(|mut g| g.take())
    }
}

async fn read_clipboard() -> Result<String, String> {
    let candidates: &[(&str, &[&str])] = &[
        ("wl-paste", &["--no-newline"]),
        ("xclip", &["-selection", "clipboard", "-o"]),
    ];
    let mut last_err = String::new();
    for (cmd, args) in candidates {
        match tokio::process::Command::new(cmd).args(*args).output().await {
            Ok(out) if out.status.success() => {
                let s = String::from_utf8_lossy(&out.stdout).into_owned();
                return Ok(s);
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

async fn write_token_file(token: &str) -> std::io::Result<()> {
    let Some(path) = token_file_path() else {
        return Err(std::io::Error::other("no config dir"));
    };
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, format!("{token}\n")).await?;
    // Best-effort: tighten permissions so other users can't read the token.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = tokio::fs::metadata(&path).await?.permissions();
        perms.set_mode(0o600);
        let _ = tokio::fs::set_permissions(&path, perms).await;
    }
    Ok(())
}

static REFRESH: RefreshSignal = RefreshSignal::new();

#[derive(Debug)]
struct RefreshSignal {
    notify: std::sync::OnceLock<tokio::sync::Notify>,
}

impl RefreshSignal {
    const fn new() -> Self {
        Self {
            notify: std::sync::OnceLock::new(),
        }
    }
    fn cell(&self) -> &tokio::sync::Notify {
        self.notify.get_or_init(tokio::sync::Notify::new)
    }
    fn notify(&self) {
        self.cell().notify_waiters();
    }
    async fn wait(&self) {
        self.cell().notified().await;
    }
}

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

    let mut settings = load_settings();
    let mut last = GitlabInfo {
        auth: if settings.token.is_some() {
            AuthState::Authenticated
        } else {
            AuthState::Missing
        },
        host: settings.host.clone(),
        ..GitlabInfo::default()
    };
    // Emit initial "missing token" / placeholder state.
    if tx.send(last.clone()).is_err() {
        return;
    }

    loop {
        // Re-read token each tick so the user's token-file edits take effect
        // without a restart.
        settings = load_settings();
        let mut info = match fetch_todos(&client, &settings).await {
            Ok((todos, total)) => GitlabInfo {
                auth: AuthState::Authenticated,
                todos,
                total,
                error: None,
                host: settings.host.clone(),
            },
            Err(FetchError::Missing) => GitlabInfo {
                auth: AuthState::Missing,
                todos: Vec::new(),
                total: 0,
                error: None,
                host: settings.host.clone(),
            },
            Err(FetchError::Invalid) => GitlabInfo {
                auth: AuthState::Invalid,
                todos: Vec::new(),
                total: 0,
                error: Some("Token rejected by GitLab".to_string()),
                host: settings.host.clone(),
            },
            Err(FetchError::Network(msg)) => GitlabInfo {
                auth: last.auth.clone(),
                todos: last.todos.clone(),
                total: last.total,
                error: Some(msg),
                host: settings.host.clone(),
            },
        };

        // A failed clipboard paste (or one with extra context) wins over a
        // generic "no token" message: it's the most recent user-visible action.
        if let Some(msg) = CLIPBOARD_ERROR.take() {
            info.error = Some(msg);
        }

        if info != last {
            last = info.clone();
            if tx.send(info).is_err() {
                return;
            }
        }

        let interval = if PANEL.is_open() {
            POLL_INTERVAL_OPEN
        } else {
            POLL_INTERVAL_CLOSED
        };
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            () = PANEL.changed() => {}
            () = REFRESH.wait() => {}
        }
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

    #[test]
    fn parse_total_uses_x_total_header_when_present() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(
            "x-total",
            "42".parse()
                .unwrap_or(reqwest::header::HeaderValue::from_static("0")),
        );
        assert_eq!(parse_total(&h, 7), 42);
    }

    #[test]
    fn parse_total_falls_back_to_count_when_header_missing() {
        let h = reqwest::header::HeaderMap::new();
        assert_eq!(parse_total(&h, 7), 7);
    }
}
