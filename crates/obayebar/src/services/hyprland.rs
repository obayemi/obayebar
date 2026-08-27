//! The bar's half of the Hyprland IPC: workspaces, windows and the event
//! stream.
//!
//! The transport, monitor detection and layer observation live in
//! `obayebar_core::hypr`, so the wallpaper renderer and the lock screen can ask
//! the same questions without linking iced. Everything here is about what the
//! *bar* renders.

use futures_util::Stream;
use obayebar_core::hypr::{parse_lenient, query_or_log, socket_dir, MonitorGeom};
use serde::Deserialize;
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, BufReader};
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
/// Read a full state snapshot from Hyprland.
///
/// `None` means "we could not find out", which is deliberately distinct from
/// "nothing is connected". The previous `unwrap_or_default()` collapsed the
/// two, so a momentary socket hiccup or a Hyprland restart produced a state
/// with zero monitors — and the reconciler dutifully closed every bar. With
/// `StartMode::Active` that also emptied `units` and made layershellev call
/// `signal.stop()`, killing the process outright.
async fn fetch_full_state() -> Option<HyprState> {
    // `monitors()` carries the "an empty list means we asked at a bad moment"
    // guard, so a transient hiccup arrives here as `None` — unknown — rather
    // than as an empty state the reconciler would action by closing every bar.
    let monitors = obayebar_core::hypr::monitors().await?;

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

/// Send a dispatcher and log whatever the compositor says back.
///
/// Detached, because every caller is a click handler that must not block the
/// UI on the socket. What it no longer does is throw the reply away: a
/// dispatcher Hyprland rejects is the whole reason workspace clicks went quiet.
fn dispatch(dispatcher: String) {
    tokio::spawn(async move {
        if let Err(e) = obayebar_core::hypr::dispatch(&dispatcher).await {
            log::warn!("hyprland: {e}");
        }
    });
}

/// Escape the ECMAScript regex metacharacters in `literal`.
///
/// Window selectors are regexes, so an unescaped class like
/// `org.mozilla.firefox` matches more than the one window meant, and a name
/// carrying a bracket or a paren does not compile at all.
fn escape_regex(literal: &str) -> String {
    let mut escaped = String::with_capacity(literal.len());
    for ch in literal.chars() {
        if r"\^$.|?*+()[]{}".contains(ch) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// The dispatcher that focuses workspace `id`.
fn focus_workspace(id: i32) -> String {
    format!("hl.dsp.focus({{workspace = {id}}})")
}

/// The dispatcher that focuses a window whose class matches `pattern`.
///
/// The selector goes in a Lua long string so `pattern` keeps a single layer of
/// escaping — a quoted Lua string would need every backslash of the regex
/// doubled. Hyprland matches a class selector against the whole class, so a
/// pattern meant to match part of one has to spell that out.
fn focus_window_class_matching(pattern: &str) -> String {
    format!("hl.dsp.focus({{window = [==[class:{pattern}]==]}})")
}

pub fn switch_workspace(id: i32) {
    dispatch(focus_workspace(id));
}

/// Focus a window whose class contains `app_name`, case insensitively.
///
/// Notifications name their sender loosely — `Firefox` for `firefox`, `Spotify`
/// for `spotify` — so this matches anywhere in the class rather than demanding
/// the whole of it.
pub fn focus_window(app_name: &str) {
    dispatch(focus_window_class_matching(&format!(
        "(?i).*{}.*",
        escape_regex(app_name)
    )));
}

/// Focus the most recent window whose class is exactly `class` (case
/// insensitive). Focusing a window switches to its workspace, which is what we
/// want after launching a browser.
pub fn focus_window_class(class: &str) {
    dispatch(focus_window_class_matching(&format!(
        "^(?i){}$",
        escape_regex(class)
    )));
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
    use super::{
        classify_event, escape_regex, focus_window_class_matching, focus_workspace, EventAction,
    };

    #[test]
    fn a_workspace_is_focused_through_the_lua_dispatcher() {
        // Hyprland 0.56 evaluates the tail of `dispatch` as Lua, so the old
        // `workspace 3` line came back as a syntax error and the click was lost.
        assert_eq!(focus_workspace(3), "hl.dsp.focus({workspace = 3})");
    }

    #[test]
    fn a_class_selector_is_anchored_and_escaped() {
        assert_eq!(
            focus_window_class_matching(&format!("^(?i){}$", escape_regex("org.mozilla.firefox"))),
            r"hl.dsp.focus({window = [==[class:^(?i)org\.mozilla\.firefox$]==]})"
        );
    }

    #[test]
    fn regex_metacharacters_in_a_name_stay_literal() {
        assert_eq!(escape_regex("Foo (beta) [1]+"), r"Foo \(beta\) \[1\]\+");
    }

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
