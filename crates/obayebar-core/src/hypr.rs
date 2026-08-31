//! Hyprland IPC: the transport, monitor detection, and layer-surface
//! observation.
//!
//! Split out of the bar so the wallpaper renderer and the lock screen can ask
//! the same questions without linking a GUI stack. What stays bar-side is
//! everything about workspaces and windows; what lives here is what any
//! obayebar process might need — which monitors exist, what is actually mapped
//! on them, and which events say the monitor set changed.
//!
//! Both an async and a blocking monitor query are provided. That is not
//! duplication for its own sake: the bar already runs a tokio reactor and wants
//! the async one, while the lock screen is a one-shot that would otherwise
//! spin up a whole runtime to make a single request and exit.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
#[cfg(feature = "async")]
use tokio::io::AsyncWriteExt;
#[cfg(feature = "async")]
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
    #[error("hyprland refused {command}: {reply}")]
    Refused { command: String, reply: String },
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

/// Send `command` over the control socket and return the reply verbatim.
///
/// # Errors
///
/// Returns [`IpcError`] naming the step that failed: no socket directory,
/// connect, write, read, or a non-UTF-8 reply.
#[cfg(feature = "async")]
pub async fn request(command: &str) -> Result<String, IpcError> {
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
    String::from_utf8(buf).map_err(|_| IpcError::NotUtf8 {
        command: command.to_string(),
    })
}

/// Send `command` over the control socket and deserialize the reply.
///
/// # Errors
///
/// Returns [`IpcError`] naming the step that failed: no socket directory,
/// connect, write, read, non-UTF-8 reply, or a reply that did not match `T`.
#[cfg(feature = "async")]
pub async fn query_json<T: serde::de::DeserializeOwned>(command: &str) -> Result<T, IpcError> {
    let text = request(command).await?;
    serde_json::from_str(&text).map_err(|source| IpcError::Parse {
        command: command.to_string(),
        source,
    })
}

/// Run a dispatcher, where `dispatcher` is the Lua call itself — e.g.
/// `hl.dsp.focus({workspace = 3})`.
///
/// Hyprland 0.56 moved dispatchers to Lua: everything after `dispatch ` is now
/// evaluated as `return hl.dispatch(...)`, so the line the bar used to send,
/// `dispatch workspace 3`, is parsed as Lua and rejected with a syntax error.
/// Clicking a workspace did nothing, and nothing said why, because the reply
/// was never read.
///
/// # Errors
///
/// Everything [`request`] can fail with, plus [`IpcError::Refused`] when the
/// compositor answers anything other than `ok` — an unknown dispatcher, a
/// selector that matched no window, or Lua that does not parse.
#[cfg(feature = "async")]
pub async fn dispatch(dispatcher: &str) -> Result<(), IpcError> {
    let command = format!("dispatch {dispatcher}");
    let reply = request(&command).await?;
    if reply.trim() == "ok" {
        Ok(())
    } else {
        Err(IpcError::Refused {
            command,
            reply: reply.trim().to_string(),
        })
    }
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
#[cfg(feature = "async")]
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
#[cfg(feature = "async")]
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
    // Same guard as the async twin, and it warns for the same reason. The
    // caller still gets a list — an empty one — because a lock screen with no
    // per-monitor backgrounds is a far better outcome than no lock at all.
    Ok(reject_empty(parse_lenient(values)).unwrap_or_default())
}

/// Whether a Hyprland event name reports a change to the set of monitors.
///
/// One list, shared: the bar reconciles its surfaces on these and the
/// wallpaper renderer runs a selection pass on them, and two copies would
/// drift the moment Hyprland renames one. `monitoradded` sits beside
/// `monitoraddedv2` because the compositor emits both and relying on one
/// couples us to that staying true.
#[must_use]
pub fn is_monitor_event(event_name: &str) -> bool {
    matches!(
        event_name,
        "monitoradded"
            | "monitoraddedv2"
            | "monitorremoved"
            | "monitorremovedv2"
            // A layout change can enable, disable or move an output with no
            // add/remove pair.
            | "monitorlayoutchanged"
            // A reload can redefine the monitors wholesale.
            | "configreloaded"
    )
}

/// What one read of the event socket produced.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EventBatch {
    /// Event names, in arrival order, with their payload stripped.
    pub names: Vec<String>,
    /// The compositor closed the socket, or it failed: this reader is done and
    /// the caller has to connect again.
    pub closed: bool,
}

impl EventBatch {
    /// Whether anything in this batch changed the monitor set.
    #[must_use]
    pub fn has_monitor_event(&self) -> bool {
        self.names.iter().any(|name| is_monitor_event(name))
    }
}

/// A non-blocking reader over Hyprland's event socket, for processes with no
/// async runtime.
///
/// The bar reads the same socket through tokio. This exists so the wallpaper
/// renderer — a synchronous calloop program — can watch the same events
/// without linking a runtime: the file descriptor goes into the event loop
/// alongside the wayland connection, and [`read_available`](Self::read_available)
/// drains whatever arrived.
#[derive(Debug)]
pub struct EventSocket {
    stream: std::os::unix::net::UnixStream,
    /// Bytes read that did not end on a line boundary, kept for the next read.
    ///
    /// A `RefCell` because calloop hands a source back to its callback behind
    /// a wrapper that derefs immutably only — it is what stops a callback
    /// closing the file descriptor out from under the loop — so the buffer
    /// cannot be reached through a `&mut self`.
    pending: std::cell::RefCell<Vec<u8>>,
}

impl EventSocket {
    /// Connect to the event socket of the running instance.
    ///
    /// # Errors
    ///
    /// Returns [`IpcError`] when we are not running under Hyprland or the
    /// socket refuses the connection.
    pub fn connect() -> Result<Self, IpcError> {
        let dir = socket_dir().ok_or(IpcError::NoSocketDir)?;
        let path = dir.join(".socket2.sock");
        let stream =
            std::os::unix::net::UnixStream::connect(&path).map_err(|source| IpcError::Connect {
                path: path.display().to_string(),
                source,
            })?;
        // Non-blocking is what lets an event loop poll this alongside its other
        // sources; a blocking read here would park the whole process.
        stream
            .set_nonblocking(true)
            .map_err(|source| IpcError::Connect {
                path: path.display().to_string(),
                source,
            })?;
        Ok(Self {
            stream,
            pending: std::cell::RefCell::new(Vec::new()),
        })
    }

    /// Drain everything the socket has buffered, without blocking.
    pub fn read_available(&self) -> EventBatch {
        use std::io::Read as _;

        let mut batch = EventBatch::default();
        let mut chunk = [0u8; 4096];
        loop {
            // `&UnixStream` implements `Read`, so the socket itself needs no
            // mutable access either.
            match (&self.stream).read(&mut chunk) {
                Ok(0) => {
                    batch.closed = true;
                    break;
                }
                Ok(read) => {
                    let fresh = chunk.get(..read).unwrap_or(&[]);
                    batch
                        .names
                        .append(&mut take_event_names(&mut self.pending.borrow_mut(), fresh));
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                // A signal cut the read short; the loop goes round again.
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    log::warn!("hyprland: reading the event socket ({e})");
                    batch.closed = true;
                    break;
                }
            }
        }
        batch
    }
}

impl std::os::fd::AsFd for EventSocket {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        std::os::fd::AsFd::as_fd(&self.stream)
    }
}

#[cfg(test)]
impl EventSocket {
    /// Wrap an already-connected stream, so the drain loop can be exercised
    /// over a socket pair instead of a compositor.
    fn wrapping(stream: std::os::unix::net::UnixStream) -> std::io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            pending: std::cell::RefCell::new(Vec::new()),
        })
    }
}

/// Take the event names off every whole line in `pending` + `fresh`, leaving
/// any partial tail behind.
///
/// A read returns whatever bytes happen to be in the kernel buffer, which cuts
/// lines in half often enough to matter: dropping the tail would lose an event
/// entirely, and a hotplug burst is exactly when the socket is busiest.
fn take_event_names(pending: &mut Vec<u8>, fresh: &[u8]) -> Vec<String> {
    pending.extend_from_slice(fresh);
    let mut names = Vec::new();
    while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
        let line: Vec<u8> = pending.drain(..=end).collect();
        let text = String::from_utf8_lossy(&line);
        if let Some((name, _payload)) = text.trim_end().split_once(">>") {
            names.push(name.to_string());
        }
    }
    names
}

/// One entry in Hyprland's `j/layers` output.
#[cfg(feature = "async")]
#[derive(Debug, Clone, Deserialize)]
struct LayerEntry {
    #[serde(default)]
    namespace: String,
}

/// The `levels` map Hyprland nests layer surfaces under, keyed by layer level.
#[cfg(feature = "async")]
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
#[cfg(feature = "async")]
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

    #[test]
    fn every_topology_event_is_a_monitor_event() {
        // A miss here strands a freshly plugged screen: the bar never spawns a
        // bar on it and the wallpaper never picks it a picture.
        for name in [
            "monitoradded",
            "monitoraddedv2",
            "monitorremoved",
            "monitorremovedv2",
            "monitorlayoutchanged",
            "configreloaded",
        ] {
            assert!(is_monitor_event(name), "{name}");
        }
    }

    #[test]
    fn window_noise_is_not_a_monitor_event() {
        for name in ["windowtitle", "activewindow", "workspace", "focusedmon", ""] {
            assert!(!is_monitor_event(name), "{name}");
        }
    }

    #[test]
    fn whole_lines_yield_their_event_names() {
        let mut pending = Vec::new();
        let names = take_event_names(&mut pending, b"monitoradded>>DP-3\nworkspace>>2\n");
        assert_eq!(names, vec!["monitoradded", "workspace"]);
        assert_eq!(pending, Vec::<u8>::new());
    }

    #[test]
    fn a_line_split_across_reads_is_not_lost() {
        // The kernel hands over whatever is buffered, and a hotplug burst is
        // exactly when a line gets cut in half.
        let mut pending = Vec::new();
        assert_eq!(
            take_event_names(&mut pending, b"monitorad"),
            Vec::<String>::new()
        );
        let names = take_event_names(&mut pending, b"dedv2>>1,DP-3,desc\n");
        assert_eq!(names, vec!["monitoraddedv2"]);
        assert!(EventBatch {
            names,
            closed: false
        }
        .has_monitor_event());
    }

    #[test]
    fn a_line_without_a_payload_separator_is_dropped() {
        let mut pending = Vec::new();
        assert_eq!(
            take_event_names(&mut pending, b"garbage\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_quiet_socket_returns_without_blocking() {
        // The whole event loop sits behind this call. A read that waited for
        // data would park the process with a wallpaper half assigned.
        let (ours, _theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let socket = EventSocket::wrapping(ours).unwrap();
        assert_eq!(socket.read_available(), EventBatch::default());
    }

    #[test]
    fn events_are_drained_across_several_reads() {
        use std::io::Write as _;

        let (ours, mut theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let socket = EventSocket::wrapping(ours).unwrap();

        theirs
            .write_all(b"monitoradded>>DP-3\nwindowtitle>>x")
            .unwrap();
        let first = socket.read_available();
        assert_eq!(first.names, vec!["monitoradded"]);
        assert!(first.has_monitor_event());
        assert!(!first.closed);

        theirs.write_all(b"\nmonitorremoved>>DP-3\n").unwrap();
        let second = socket.read_available();
        assert_eq!(second.names, vec!["windowtitle", "monitorremoved"]);
        assert!(second.has_monitor_event());
    }

    #[test]
    fn a_compositor_that_goes_away_is_reported_as_closed() {
        // The caller drops the source and reconnects on this; missing it would
        // spin the event loop on a dead descriptor.
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let socket = EventSocket::wrapping(ours).unwrap();
        drop(theirs);
        assert!(socket.read_available().closed);
    }
}
