//! Hyprland IPC: the transport, monitor detection, and layer-surface
//! observation.
//!
//! Split out of the bar so the wallpaper renderer and the lock screen can ask
//! the same questions without linking a GUI stack. What stays bar-side is
//! everything about workspaces, windows and the event stream; what lives here
//! is what any obayebar process might need — which monitors exist, and what is
//! actually mapped on them.
//!
//! Both an async and a blocking monitor query are provided. That is not
//! duplication for its own sake: the bar already runs a tokio reactor and wants
//! the async one, while the lock screen is a one-shot that would otherwise
//! spin up a whole runtime to make a single request and exit.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

/// How long the blocking query waits on a compositor that accepted the
/// connection but never answers. The lock screen sits behind this, so it must
/// fail fast rather than leave the user staring at an unlocked desktop.
const BLOCKING_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MonitorWorkspace {
    pub id: i32,
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
    /// Human-readable panel description, e.g.
    /// `Dell Inc. DELL U2518D 3C4YP95TBQ5L`.
    ///
    /// This is the only thing that distinguishes two identical displays: on
    /// this machine DP-9 and DP-10 are the same Dell model and differ solely by
    /// the serial at the end. Port names get reshuffled across DPMS cycles, so
    /// anything that must follow a physical panel keys on this instead.
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub serial: String,
    /// Hyprland lists disabled outputs too; they have no surface to place
    /// anything on.
    #[serde(default)]
    pub disabled: bool,
    /// `"none"` when this monitor is not mirroring another. A mirror shows the
    /// source's contents, so giving it its own wallpaper is wasted work.
    #[serde(rename = "mirrorOf", default)]
    pub mirror_of: String,
}

impl MonitorInfo {
    /// The key to remember this monitor by across disconnects.
    ///
    /// The description when there is one, because it names the physical panel:
    /// port names get reshuffled across DPMS cycles, so a wallpaper remembered
    /// against `DP-9` is lost the moment the same screen comes back as `DP-10`.
    /// Falls back to the port name, which is at least unique per compositor.
    #[must_use]
    pub fn stable_key(&self) -> String {
        let described = self.description.trim();
        if described.is_empty() {
            self.name.clone()
        } else {
            described.to_string()
        }
    }

    /// Whether this monitor can meaningfully carry its own content.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        !self.disabled && (self.mirror_of.is_empty() || self.mirror_of == "none")
    }
}

/// Physical geometry of a connected monitor, used for sizing overlays that
/// need to know the screen dimensions (e.g. the notification popup cap).
#[derive(Debug, Clone, Copy)]
pub struct MonitorGeom {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub transform: i32,
}

impl MonitorGeom {
    /// Pixel dimensions after applying the output transform.
    ///
    /// The odd transforms (90°, 270°, and their flipped twins) swap the axes.
    /// Anything sizing a buffer for this output needs the post-transform
    /// figure, or a rotated monitor gets a wallpaper with the dimensions
    /// transposed.
    #[must_use]
    pub const fn oriented_size(&self) -> (u32, u32) {
        if self.transform % 2 == 0 {
            (self.width, self.height)
        } else {
            (self.height, self.width)
        }
    }
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

/// The per-instance socket directory, or `None` when we are not running under
/// Hyprland.
///
/// Always resolved through `HYPRLAND_INSTANCE_SIGNATURE` rather than by
/// globbing: a machine can hold several instance directories at once, and a
/// stale one keeps a live-looking socket.
#[must_use]
pub fn socket_dir() -> Option<PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let xdg = std::env::var("XDG_RUNTIME_DIR").ok()?;
    Some(PathBuf::from(xdg).join("hypr").join(sig))
}

/// Send `command` over the control socket and deserialize the reply.
///
/// # Errors
///
/// Returns [`IpcError`] naming the step that failed: no socket directory,
/// connect, write, read, non-UTF-8 reply, or a reply that did not match `T`.
pub async fn query_json<T: serde::de::DeserializeOwned>(command: &str) -> Result<T, IpcError> {
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

/// The blocking twin of [`query_json`], for one-shot processes.
///
/// Shuts down only the write half — a full shutdown would close the read side
/// before the reply arrives. The reply is collected as bytes and converted
/// afterwards so a non-UTF-8 answer still reports as [`IpcError::NotUtf8`]
/// rather than as a read error.
///
/// # Errors
///
/// Same set as [`query_json`].
pub fn query_json_blocking<T: serde::de::DeserializeOwned>(command: &str) -> Result<T, IpcError> {
    use std::io::{Read as _, Write as _};

    let dir = socket_dir().ok_or(IpcError::NoSocketDir)?;
    let sock_path = dir.join(".socket.sock");
    let mut stream = std::os::unix::net::UnixStream::connect(&sock_path).map_err(|source| {
        IpcError::Connect {
            path: sock_path.display().to_string(),
            source,
        }
    })?;
    let _ = stream.set_read_timeout(Some(BLOCKING_TIMEOUT));
    let _ = stream.set_write_timeout(Some(BLOCKING_TIMEOUT));

    stream
        .write_all(command.as_bytes())
        .map_err(|source| IpcError::Write {
            command: command.to_string(),
            source,
        })?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|source| IpcError::Write {
            command: command.to_string(),
            source,
        })?;

    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
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
pub async fn query_or_log<T: serde::de::DeserializeOwned>(command: &str) -> Option<T> {
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
#[must_use]
pub fn parse_lenient<T: serde::de::DeserializeOwned>(values: Vec<serde_json::Value>) -> Vec<T> {
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

/// An empty monitor list from a live compositor means we asked at a bad
/// moment, not that the machine has no displays. Callers must not action it.
fn reject_empty(monitors: Vec<MonitorInfo>) -> Option<Vec<MonitorInfo>> {
    if monitors.is_empty() {
        log::warn!("hyprland: j/monitors returned no usable monitors, treating state as unknown");
        return None;
    }
    Some(monitors)
}

/// Every monitor Hyprland currently knows about.
///
/// `None` means the query failed or came back empty, both of which mean
/// "unknown" rather than "none connected".
pub async fn monitors() -> Option<Vec<MonitorInfo>> {
    let values: Vec<serde_json::Value> = query_or_log("j/monitors").await?;
    reject_empty(parse_lenient(values))
}

/// The blocking twin of [`monitors`], for one-shot processes.
///
/// # Errors
///
/// Returns [`IpcError`] when the compositor could not be reached or understood.
/// An empty-but-successful reply is reported as `Ok(vec![])`; callers that need
/// the "unknown" distinction should treat both as "place nothing per-monitor".
pub fn monitors_blocking() -> Result<Vec<MonitorInfo>, IpcError> {
    let values: Vec<serde_json::Value> = query_json_blocking("j/monitors")?;
    Ok(parse_lenient(values))
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
/// This is the only placement feedback available to a client using
/// `OutputOption::OutputName`: that resolves through layershellev's own
/// output-name cache and, on a miss, silently creates the surface with no
/// output at all — the compositor then puts it on the focused monitor and
/// nothing is reported back. Asking Hyprland directly is what turns bar
/// placement from a belief into an observation.
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn geom(width: u32, height: u32, transform: i32) -> MonitorGeom {
        MonitorGeom {
            width,
            height,
            scale: 1.0,
            transform,
        }
    }

    #[test]
    fn even_transforms_keep_the_axes() {
        // 0 = normal, 2 = 180°, 4/6 = flipped variants that do not rotate.
        for t in [0, 2, 4, 6] {
            assert_eq!(geom(2560, 1440, t).oriented_size(), (2560, 1440), "{t}");
        }
    }

    #[test]
    fn odd_transforms_swap_the_axes() {
        // 1 = 90°, 3 = 270°, 5/7 = flipped-and-rotated.
        for t in [1, 3, 5, 7] {
            assert_eq!(geom(2560, 1440, t).oriented_size(), (1440, 2560), "{t}");
        }
    }

    #[test]
    fn usable_excludes_disabled_and_mirrors() {
        let mut m = MonitorInfo {
            name: "DP-1".to_string(),
            mirror_of: "none".to_string(),
            ..MonitorInfo::default()
        };
        assert!(m.is_usable());

        m.disabled = true;
        assert!(!m.is_usable(), "a disabled output has nothing to draw on");

        m.disabled = false;
        m.mirror_of = "DP-2".to_string();
        assert!(!m.is_usable(), "a mirror shows its source's contents");
    }

    #[test]
    fn absent_mirror_of_is_treated_as_not_mirroring() {
        // Older Hyprland builds omit the field entirely, so serde leaves it "".
        let m = MonitorInfo {
            name: "DP-1".to_string(),
            ..MonitorInfo::default()
        };
        assert!(m.is_usable());
    }

    #[test]
    fn monitor_info_parses_the_fields_we_added() {
        let raw = serde_json::json!({
            "name": "DP-10",
            "description": "Dell Inc. DELL U2518D 3C4YP95TBJ5L",
            "serial": "3C4YP95TBJ5L",
            "activeWorkspace": { "id": 3 },
            "width": 2560,
            "height": 1440,
            "scale": 1.0,
            "transform": 0,
            "disabled": false,
            "mirrorOf": "none"
        });
        let m: MonitorInfo = serde_json::from_value(raw).unwrap();
        assert_eq!(m.description, "Dell Inc. DELL U2518D 3C4YP95TBJ5L");
        assert_eq!(m.serial, "3C4YP95TBJ5L");
        assert_eq!(m.mirror_of, "none");
        assert!(m.is_usable());
    }

    #[test]
    fn parse_lenient_keeps_the_good_entries() {
        // The whole point: one monitor in a shape we do not model must cost us
        // that monitor, not all of them.
        let values = vec![
            serde_json::json!({ "name": "DP-1", "activeWorkspace": { "id": 1 } }),
            serde_json::json!({ "name": 42, "activeWorkspace": { "id": 2 } }),
            serde_json::json!({ "name": "DP-2", "activeWorkspace": { "id": 3 } }),
        ];
        let parsed: Vec<MonitorInfo> = parse_lenient(values);
        let names: Vec<&str> = parsed.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["DP-1", "DP-2"]);
    }

    #[test]
    fn empty_monitor_list_reads_as_unknown() {
        assert!(reject_empty(Vec::new()).is_none());
        assert_eq!(
            reject_empty(vec![MonitorInfo::default()]).map(|v| v.len()),
            Some(1)
        );
    }
}
