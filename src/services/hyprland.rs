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
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub scale: f32,
    #[serde(default)]
    pub transform: i32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MonitorWorkspace {
    pub id: i32,
}

/// Physical geometry of a connected monitor, used for sizing overlays
/// that need to know the screen dimensions (e.g. notification popup cap).
#[derive(Debug, Clone, Copy)]
pub struct MonitorGeom {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub transform: i32,
}

/// All state fetched from Hyprland in one batch
#[derive(Debug, Clone)]
pub struct HyprState {
    pub monitors: Vec<String>,
    pub focused_monitor: String,
    pub monitor_geoms: HashMap<String, MonitorGeom>,
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
    let monitor_geoms: HashMap<String, MonitorGeom> = monitors
        .iter()
        .map(|m| {
            (
                m.name.clone(),
                MonitorGeom {
                    width: m.width,
                    height: m.height,
                    scale: if m.scale > 0.0 { m.scale } else { 1.0 },
                    transform: m.transform,
                },
            )
        })
        .collect();

    HyprState {
        monitors: monitor_names,
        focused_monitor,
        monitor_geoms,
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

pub fn focus_window(app_name: &str) {
    let app_name = app_name.to_lowercase();
    tokio::spawn(async move {
        let Some(dir) = socket_dir() else {
            return;
        };
        let sock_path = dir.join(".socket.sock");
        if let Ok(mut stream) = UnixStream::connect(&sock_path).await {
            let cmd = format!("dispatch focuswindow {app_name}");
            let _ = stream.write_all(cmd.as_bytes()).await;
        }
    });
}

/// Focus the most recent window whose initial class matches `class` (case
/// insensitive). Hyprland's `focuswindow` switches to the window's workspace,
/// which is what we want after launching a browser.
pub fn focus_window_class(class: &str) {
    let class = class.to_string();
    tokio::spawn(async move {
        let Some(dir) = socket_dir() else {
            return;
        };
        let sock_path = dir.join(".socket.sock");
        if let Ok(mut stream) = UnixStream::connect(&sock_path).await {
            let cmd = format!("dispatch focuswindow class:^(?i){class}$");
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
                            monitor_geoms: HashMap::new(),
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
                // Loop until an event `classify_event` cares about, so noise
                // (windowtitle, submap, activewindowv2, …) never wakes the UI.
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line).await {
                        Ok(0) | Err(_) => {
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            return Some((
                                HyprEvent::State(fetch_full_state().await),
                                State::Starting,
                            ));
                        }
                        Ok(_) => {
                            if let Some(event) = parse_event(line.trim()).await {
                                return Some((event, State::Streaming(reader)));
                            }
                        }
                    }
                }
            }
        }
    })
}

async fn parse_event(line: &str) -> Option<HyprEvent> {
    let (event_name, data) = line.split_once(">>")?;

    match classify_event(event_name) {
        // Active window: parse class,title directly from event data — no need
        // to re-query the socket. Event payload is "WINDOWCLASS,WINDOWTITLE".
        EventAction::ActiveWindow => {
            let win = data.split_once(',').map(|(class, title)| WindowInfo {
                class: class.to_string(),
                title: title.to_string(),
            });
            Some(HyprEvent::ActiveWindow(win.filter(|w| !w.class.is_empty())))
        }
        EventAction::Refresh => Some(HyprEvent::State(fetch_full_state().await)),
        EventAction::Ignore => {
            // Logged so a Hyprland rename of a handled event shows up as a
            // named unknown instead of silently becoming "we stopped
            // refreshing". Trace level: this fires on every title change.
            log::trace!("hyprland: ignoring event {event_name}");
            None
        }
    }
}

/// What a Hyprland IPC event name should make the bar do. Split out from the
/// socket handling so the arm list is testable without a compositor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventAction {
    /// Parse the active window straight out of the event payload.
    ActiveWindow,
    /// Re-query full state: something we render changed.
    Refresh,
    /// High-frequency noise we deliberately drop.
    Ignore,
}

/// Map a Hyprland event name to the action it requires.
///
/// The refresh set is everything that can change the workspace list, which
/// monitor a workspace lives on, the focused monitor, or the set of connected
/// monitors. `monitoradded` is listed alongside `monitoraddedv2`: Hyprland
/// emits both, and relying on only one couples us to that staying true.
#[must_use]
pub fn classify_event(event_name: &str) -> EventAction {
    match event_name {
        "activewindow" => EventAction::ActiveWindow,
        "workspace"
        | "workspacev2"
        | "createworkspace"
        | "createworkspacev2"
        | "destroyworkspace"
        | "destroyworkspacev2"
        // Moving a workspace between monitors changes exactly the
        // monitor->workspace mapping the indicator renders.
        | "moveworkspace"
        | "moveworkspacev2"
        | "focusedmon"
        | "focusedmonv2"
        | "openwindow"
        | "closewindow"
        | "movewindow"
        | "movewindowv2"
        | "monitoradded"
        | "monitoraddedv2"
        | "monitorremoved"
        | "monitorremovedv2"
        // A layout change can enable/disable or move an output without an
        // add/remove pair.
        | "monitorlayoutchanged"
        // A reload can redefine monitors and workspace rules wholesale.
        | "configreloaded"
        | "changefloatingmode"
        | "urgent" => EventAction::Refresh,
        // Title changes, submap changes, activewindowv2 (a duplicate of
        // activewindow), and everything else we do not render.
        _ => EventAction::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_event, EventAction};

    #[test]
    fn active_window_is_parsed_from_the_payload() {
        assert_eq!(classify_event("activewindow"), EventAction::ActiveWindow);
    }

    #[test]
    fn monitor_topology_events_force_a_refresh() {
        // These drive bar placement, so a miss here strands a monitor without
        // a bar until some unrelated event happens to refresh.
        for name in [
            "monitoradded",
            "monitoraddedv2",
            "monitorremoved",
            "monitorremovedv2",
            "monitorlayoutchanged",
            "configreloaded",
        ] {
            assert_eq!(classify_event(name), EventAction::Refresh, "{name}");
        }
    }

    #[test]
    fn workspace_moves_between_monitors_force_a_refresh() {
        // The monitor->workspace mapping is what the indicator renders.
        assert_eq!(classify_event("moveworkspace"), EventAction::Refresh);
        assert_eq!(classify_event("moveworkspacev2"), EventAction::Refresh);
    }

    #[test]
    fn workspace_and_window_events_force_a_refresh() {
        for name in [
            "workspace",
            "workspacev2",
            "createworkspace",
            "createworkspacev2",
            "destroyworkspace",
            "destroyworkspacev2",
            "focusedmon",
            "focusedmonv2",
            "openwindow",
            "closewindow",
            "movewindow",
            "movewindowv2",
            "changefloatingmode",
            "urgent",
        ] {
            assert_eq!(classify_event(name), EventAction::Refresh, "{name}");
        }
    }

    #[test]
    fn high_frequency_noise_is_ignored() {
        // activewindowv2 duplicates activewindow; the rest we do not render.
        for name in ["windowtitle", "windowtitlev2", "activewindowv2", "submap"] {
            assert_eq!(classify_event(name), EventAction::Ignore, "{name}");
        }
    }

    #[test]
    fn unknown_events_are_ignored_rather_than_refreshing() {
        assert_eq!(
            classify_event("somethinghyprlandaddedlater"),
            EventAction::Ignore
        );
        assert_eq!(classify_event(""), EventAction::Ignore);
    }
}
