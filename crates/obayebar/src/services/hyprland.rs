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

/// Why a Hyprland IPC query failed.
///
/// Every step used to end in `.ok()?`, collapsing six distinct failures into
/// one `None` that the caller then turned into an empty state. This module had
/// no log statements at all, so a compositor that had gone away, a renamed
/// socket and a JSON schema change were indistinguishable — and all three
/// presented as "no monitors are connected".
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("HYPRLAND_INSTANCE_SIGNATURE or XDG_RUNTIME_DIR is unset")]
    NoSocketDir,
    #[error("connecting to {path}")]
    Connect {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("sending {command}")]
    Write {
        command: String,
        #[source]
        source: std::io::Error,
    },
    #[error("reading the reply to {command}")]
    Read {
        command: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the reply to {command} was not valid UTF-8")]
    NotUtf8 { command: String },
    #[error("parsing the reply to {command}")]
    Parse {
        command: String,
        #[source]
        source: serde_json::Error,
    },
}

async fn query_json<T: serde::de::DeserializeOwned>(command: &str) -> Result<T, IpcError> {
    let dir = socket_dir().ok_or(IpcError::NoSocketDir)?;
    let sock_path = dir.join(".socket.sock");
    let mut stream = UnixStream::connect(&sock_path)
        .await
        .map_err(|source| IpcError::Connect {
            path: sock_path.display().to_string(),
            source,
        })?;
    stream
        .write_all(command.as_bytes())
        .await
        .map_err(|source| IpcError::Write {
            command: command.to_string(),
            source,
        })?;
    stream.shutdown().await.map_err(|source| IpcError::Write {
        command: command.to_string(),
        source,
    })?;

    let mut buf = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut buf)
        .await
        .map_err(|source| IpcError::Read {
            command: command.to_string(),
            source,
        })?;
    let text = String::from_utf8(buf).map_err(|_| IpcError::NotUtf8 {
        command: command.to_string(),
    })?;
    serde_json::from_str(&text).map_err(|source| IpcError::Parse {
        command: command.to_string(),
        source,
    })
}

/// Run a query, logging the reason on failure. `None` here means "we could not
/// find out", which callers must never action as "there is nothing".
async fn query_or_log<T: serde::de::DeserializeOwned>(command: &str) -> Option<T> {
    match query_json(command).await {
        Ok(value) => Some(value),
        Err(e) => {
            log::warn!("hyprland: {e}");
            None
        }
    }
}

/// Deserialize a JSON array element-wise, dropping the entries that fail.
///
/// `Vec<T>` as a whole would fail on a single unparseable element, so one
/// monitor Hyprland describes in a shape we do not model used to discard
/// *every* monitor — which the reconciler then read as "no monitors are
/// connected". One odd monitor should cost us that monitor, nothing more.
fn parse_lenient<T: serde::de::DeserializeOwned>(values: Vec<serde_json::Value>) -> Vec<T> {
    values
        .into_iter()
        .filter_map(|value| match serde_json::from_value(value) {
            Ok(parsed) => Some(parsed),
            Err(e) => {
                log::warn!("hyprland: skipping unparseable IPC entry: {e}");
                None
            }
        })
        .collect()
}

/// Read a full state snapshot from Hyprland.
///
/// `None` means "we could not find out", which is deliberately distinct from
/// "nothing is connected". The previous `unwrap_or_default()` collapsed the
/// two, so a momentary socket hiccup or a Hyprland restart produced a state
/// with zero monitors — and the reconciler dutifully closed every bar. With
/// `StartMode::Active` that also emptied `units` and made layershellev call
/// `signal.stop()`, killing the process outright.
async fn fetch_full_state() -> Option<HyprState> {
    let monitor_values: Vec<serde_json::Value> = query_or_log("j/monitors").await?;
    let monitors: Vec<MonitorInfo> = parse_lenient(monitor_values);
    if monitors.is_empty() {
        // Hyprland lists every enabled, connected monitor here. An empty list
        // from a compositor that is up means we asked at a bad moment, not
        // that the machine has no displays.
        log::warn!("hyprland: j/monitors returned no usable monitors, treating state as unknown");
        return None;
    }

    let workspaces: Vec<WorkspaceInfo> = query_or_log::<Vec<serde_json::Value>>("j/workspaces")
        .await
        .map(parse_lenient)
        .unwrap_or_default();
    let active_window: Option<WindowInfo> = query_or_log::<WindowInfo>("j/activewindow")
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

    Some(HyprState {
        monitors: monitor_names,
        focused_monitor,
        monitor_geoms,
        workspaces,
        active_workspaces,
        active_window,
    })
}

/// One entry in Hyprland's `j/layers` output.
#[derive(Debug, Clone, Deserialize)]
struct LayerEntry {
    #[serde(default)]
    namespace: String,
}

/// The `levels` map Hyprland nests layer surfaces under, keyed by layer level.
#[derive(Debug, Clone, Default, Deserialize)]
struct MonitorLayers {
    #[serde(default)]
    levels: HashMap<String, Vec<LayerEntry>>,
}

/// Which layer-shell namespaces are mapped on which monitor, as the compositor
/// sees it. Keyed by monitor name.
pub type LayerMap = HashMap<String, Vec<String>>;

/// Ask the compositor which layer surfaces are actually on which monitor.
///
/// This is the only placement feedback available to us: `OutputOption::
/// OutputName` resolves through layershellev's own output-name cache and, on a
/// miss, silently creates the surface with no output at all — the compositor
/// then puts it on the focused monitor and nothing is reported back. Asking
/// Hyprland directly is what turns bar placement from a belief into an
/// observation.
///
/// `None` means the query failed, which callers must treat as "unknown" and
/// never as "no layers are mapped".
pub async fn fetch_layer_namespaces() -> Option<LayerMap> {
    let raw: HashMap<String, MonitorLayers> = query_or_log("j/layers").await?;
    Some(
        raw.into_iter()
            .map(|(monitor, layers)| {
                let namespaces = layers
                    .levels
                    .into_values()
                    .flatten()
                    .map(|entry| entry.namespace)
                    .filter(|ns| !ns.is_empty())
                    .collect();
                (monitor, namespaces)
            })
            .collect(),
    )
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

/// How long to wait before retrying a failed socket connection or state read.
const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
/// Shorter pause after the event socket closes, since a compositor restart
/// usually comes back quickly.
const RECONNECT_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

pub fn stream() -> impl Stream<Item = HyprEvent> {
    futures_util::stream::unfold(State::Starting, |mut state| async move {
        // Only ever yields state we actually read. When there is nothing
        // trustworthy to report the loop retries instead of emitting — the
        // old code invented an all-empty `HyprState` here, which downstream
        // could not tell from "every monitor was just unplugged".
        loop {
            match state {
                State::Starting => {
                    let Some(dir) = socket_dir() else {
                        log::warn!(
                            "hyprland: HYPRLAND_INSTANCE_SIGNATURE or XDG_RUNTIME_DIR unset, retrying"
                        );
                        tokio::time::sleep(RETRY_DELAY).await;
                        state = State::Starting;
                        continue;
                    };

                    let Some(hypr_state) = fetch_full_state().await else {
                        tokio::time::sleep(RETRY_DELAY).await;
                        state = State::Starting;
                        continue;
                    };

                    let sock_path = dir.join(".socket2.sock");
                    match UnixStream::connect(&sock_path).await {
                        Ok(event_stream) => {
                            return Some((
                                HyprEvent::State(hypr_state),
                                State::Streaming(BufReader::new(event_stream)),
                            ));
                        }
                        Err(e) => {
                            // The snapshot is real even though the event socket
                            // is not up yet, so it is worth publishing before
                            // we retry.
                            log::warn!("hyprland: cannot open event socket ({e}), retrying");
                            tokio::time::sleep(RETRY_DELAY).await;
                            return Some((HyprEvent::State(hypr_state), State::Starting));
                        }
                    }
                }
                State::Streaming(mut reader) => {
                    // Loop until an event `classify_event` cares about, so noise
                    // (windowtitle, submap, activewindowv2, …) never wakes the UI.
                    let next = loop {
                        let mut line = String::new();
                        match reader.read_line(&mut line).await {
                            Ok(0) | Err(_) => break None,
                            Ok(_) => {
                                if let Some(event) = parse_event(line.trim()).await {
                                    break Some(event);
                                }
                            }
                        }
                    };
                    if let Some(event) = next {
                        return Some((event, State::Streaming(reader)));
                    }
                    // Socket closed. Go back to `Starting` without emitting: it
                    // re-reads the state and publishes that, so there is
                    // nothing to invent here.
                    log::warn!("hyprland: event socket closed, reconnecting");
                    tokio::time::sleep(RECONNECT_DELAY).await;
                    state = State::Starting;
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
        // A failed read is skipped rather than reported as an empty state; the
        // next event (or the next topology change) refreshes again.
        EventAction::Refresh => fetch_full_state().await.map(HyprEvent::State),
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
