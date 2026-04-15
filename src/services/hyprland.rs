use futures_util::Stream;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct WorkspaceInfo {
    pub id: i32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub windows: u32,
    #[serde(default)]
    pub monitor: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct WindowInfo {
    pub class: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MonitorInfo {
    pub name: String,
    #[serde(rename = "activeWorkspace")]
    pub active_workspace: MonitorWorkspace,
    #[serde(default)]
    pub focused: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MonitorWorkspace {
    pub id: i32,
}

/// All state fetched from Hyprland in one batch
#[derive(Debug, Clone)]
pub struct HyprState {
    pub monitors: Vec<String>,
    pub focused_monitor: String,
    pub workspaces: Vec<WorkspaceInfo>,
    pub active_workspaces: HashMap<String, i32>,
    pub active_window: Option<WindowInfo>,
}

#[derive(Debug, Clone)]
pub enum HyprEvent {
    /// Full state refresh
    State(HyprState),
    /// Active window changed
    ActiveWindow(Option<WindowInfo>),
}

fn socket_dir() -> Option<PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let xdg = std::env::var("XDG_RUNTIME_DIR").ok()?;
    Some(PathBuf::from(xdg).join("hypr").join(sig))
}

async fn query_json<T: serde::de::DeserializeOwned>(command: &str) -> Option<T> {
    let dir = socket_dir()?;
    let sock_path = dir.join(".socket.sock");
    let mut stream = UnixStream::connect(&sock_path).await.ok()?;
    stream.write_all(command.as_bytes()).await.ok()?;
    stream.shutdown().await.ok()?;

    let mut buf = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut buf)
        .await
        .ok()?;
    let text = String::from_utf8(buf).ok()?;
    serde_json::from_str(&text).ok()
}

async fn fetch_full_state() -> HyprState {
    let monitors: Vec<MonitorInfo> = query_json("j/monitors").await.unwrap_or_default();
    let workspaces: Vec<WorkspaceInfo> = query_json("j/workspaces").await.unwrap_or_default();
    let active_window: Option<WindowInfo> = query_json::<WindowInfo>("j/activewindow")
        .await
        .filter(|w| !w.class.is_empty());

    let monitor_names: Vec<String> = monitors.iter().map(|m| m.name.clone()).collect();
    let focused_monitor = monitors
        .iter()
        .find(|m| m.focused)
        .map(|m| m.name.clone())
        .or_else(|| monitor_names.first().cloned())
        .unwrap_or_default();
    let active_workspaces: HashMap<String, i32> = monitors
        .iter()
        .map(|m| (m.name.clone(), m.active_workspace.id))
        .collect();

    HyprState {
        monitors: monitor_names,
        focused_monitor,
        workspaces,
        active_workspaces,
        active_window,
    }
}

pub fn switch_workspace(id: i32) {
    tokio::spawn(async move {
        let Some(dir) = socket_dir() else {
            return;
        };
        let sock_path = dir.join(".socket.sock");
        if let Ok(mut stream) = UnixStream::connect(&sock_path).await {
            let cmd = format!("dispatch workspace {id}");
            let _ = stream.write_all(cmd.as_bytes()).await;
        }
    });
}

enum State {
    Starting,
    Streaming(BufReader<UnixStream>),
}

pub fn stream() -> impl Stream<Item = HyprEvent> {
    futures_util::stream::unfold(State::Starting, |state| async {
        match state {
            State::Starting => {
                let Some(dir) = socket_dir() else {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    return Some((
                        HyprEvent::State(HyprState {
                            monitors: Vec::new(),
                            focused_monitor: String::new(),
                            workspaces: Vec::new(),
                            active_workspaces: HashMap::new(),
                            active_window: None,
                        }),
                        State::Starting,
                    ));
                };

                // Fetch full initial state
                let hypr_state = fetch_full_state().await;

                let sock_path = dir.join(".socket2.sock");
                let Ok(event_stream) = UnixStream::connect(&sock_path).await else {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    return Some((HyprEvent::State(hypr_state), State::Starting));
                };

                let reader = BufReader::new(event_stream);
                Some((HyprEvent::State(hypr_state), State::Streaming(reader)))
            }
            State::Streaming(mut reader) => {
                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        Some((HyprEvent::State(fetch_full_state().await), State::Starting))
                    }
                    Ok(_) => {
                        let line = line.trim();
                        let event = parse_event(line).await;
                        Some((event, State::Streaming(reader)))
                    }
                }
            }
        }
    })
}

async fn parse_event(line: &str) -> HyprEvent {
    let Some((event_name, _data)) = line.split_once(">>") else {
        return HyprEvent::State(fetch_full_state().await);
    };

    match event_name {
        "activewindow" | "activewindowv2" => {
            let win: Option<WindowInfo> = query_json("j/activewindow").await;
            HyprEvent::ActiveWindow(win.filter(|w| !w.class.is_empty()))
        }
        // All workspace/monitor events trigger a full state refresh
        _ => HyprEvent::State(fetch_full_state().await),
    }
}
