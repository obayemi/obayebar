//! Unix-socket control channels, shared by every obayebar daemon and client.
//!
//! A socket rather than D-Bus because the clients otherwise need neither an
//! async runtime nor a bus connection, and every command here is a single line
//! of text. The sockets live in `$XDG_RUNTIME_DIR`, which is mode 700 and
//! cleared at logout, so a stale one cannot outlive the session that owns it.
//!
//! This module owns the socket mechanics only. Each daemon keeps its own
//! command vocabulary next to the code that executes it — except the bar's,
//! which lives here because its client ([`crate::control::BarCommand`]) must
//! be able to speak it without linking the bar's GUI stack.

use std::io::Write as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

/// Socket file the bar daemon listens on.
pub const BAR_SOCKET: &str = "bar.sock";

/// Socket file the wallpaper daemon listens on.
pub const WALLPAPER_SOCKET: &str = "wallpaper.sock";

/// What can go wrong talking to (or binding) a control socket.
#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("XDG_RUNTIME_DIR is unset")]
    NoRuntimeDir,
    /// Nothing is listening — for a client, this means the daemon is not
    /// running, which is the error worth reporting to a person.
    #[error("no daemon listening on {path}: {source}")]
    NotListening {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("another process is already listening on {0}")]
    AlreadyBound(PathBuf),
    #[error("{context}: {source}")]
    Io {
        context: String,
        source: std::io::Error,
    },
}

impl ControlError {
    fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

/// One command the bar daemon accepts.
///
/// Deliberately tiny: the bar draws the launcher itself now, so the client is
/// a keybinding shim that says "show it" and exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarCommand {
    /// Show the launcher, or hide it if it is already showing.
    LauncherToggle,
}

impl BarCommand {
    /// The wire form. Deliberately plain text so `socat` can drive it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LauncherToggle => "launcher-toggle",
        }
    }

    /// Parse a line read from the socket.
    #[must_use]
    pub fn parse(line: &str) -> Option<Self> {
        match line.trim() {
            "launcher-toggle" => Some(Self::LauncherToggle),
            _ => None,
        }
    }
}

/// Where `socket` lives, or `None` without a runtime dir.
#[must_use]
pub fn socket_path(socket: &str) -> Option<PathBuf> {
    crate::xdg::runtime_dir().map(|d| d.join(socket))
}

/// Send one line to whoever is listening on `socket`.
///
/// # Errors
///
/// [`ControlError::NotListening`] when no daemon holds the socket, which is
/// the case callers should report as "it is not running".
pub fn send_line(socket: &str, line: &str) -> Result<(), ControlError> {
    send_line_at(
        &socket_path(socket).ok_or(ControlError::NoRuntimeDir)?,
        line,
    )
}

/// [`send_line`] against an explicit path, so the mechanics can be tested
/// without mutating the process-wide `XDG_RUNTIME_DIR`.
fn send_line_at(path: &Path, line: &str) -> Result<(), ControlError> {
    let mut stream = UnixStream::connect(path).map_err(|source| ControlError::NotListening {
        path: path.to_path_buf(),
        source,
    })?;
    writeln!(stream, "{line}").map_err(|source| ControlError::io("sending the command", source))
}

/// Bind `socket`, replacing one left behind by a crashed daemon.
///
/// The listener is non-blocking, so a caller driving its own event loop can
/// poll it and a caller on tokio can adopt it with `from_std`.
///
/// # Errors
///
/// [`ControlError::AlreadyBound`] when another instance is live on the socket —
/// a real conflict, as opposed to the stale file this silently clears.
pub fn bind(socket: &str) -> Result<UnixListener, ControlError> {
    // Through the shared helper so the directory ends up mode 700 whichever
    // binary creates it first — it also holds the generated lock config, which
    // names every wallpaper path.
    crate::xdg::runtime_dir_create()
        .map_err(|source| ControlError::io("preparing the runtime directory", source))?;
    bind_at(&socket_path(socket).ok_or(ControlError::NoRuntimeDir)?)
}

/// [`bind`] against an explicit path, in a directory the caller has already
/// created.
fn bind_at(path: &Path) -> Result<UnixListener, ControlError> {
    // A socket file left by a crashed daemon would make bind fail with
    // EADDRINUSE forever. Probing it first distinguishes "stale" from "a
    // daemon is already running", which is a real error worth reporting.
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(ControlError::AlreadyBound(path.to_path_buf()));
        }
        let _ = std::fs::remove_file(path);
    }
    let listener = UnixListener::bind(path)
        .map_err(|source| ControlError::io(format!("binding {}", path.display()), source))?;
    listener
        .set_nonblocking(true)
        .map_err(|source| ControlError::io("setting the socket non-blocking", source))?;
    Ok(listener)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::io::{BufRead as _, BufReader};

    /// A scratch directory to put test sockets in.
    ///
    /// Deliberately not `XDG_RUNTIME_DIR`: the environment is process-wide, so
    /// setting it would make these tests race each other under the default
    /// parallel test runner. The path-taking `bind_at` / `send_line_at` are
    /// what the name-taking pair is built from, so this still covers them.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("obayebar_control_{tag}"));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn socket(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn bar_commands_round_trip_through_the_wire_form() {
        assert_eq!(
            BarCommand::parse(BarCommand::LauncherToggle.as_str()),
            Some(BarCommand::LauncherToggle)
        );
    }

    #[test]
    fn unknown_commands_are_rejected_rather_than_guessed() {
        for bad in ["", "launcher", "LAUNCHER-TOGGLE", "launcher-toggle now"] {
            assert_eq!(BarCommand::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn trailing_whitespace_is_tolerated() {
        // The client writes a newline; a person using socat may add more.
        assert_eq!(
            BarCommand::parse("launcher-toggle\n"),
            Some(BarCommand::LauncherToggle)
        );
    }

    #[test]
    fn a_line_sent_to_a_bound_socket_arrives() {
        let dir = ScratchDir::new("roundtrip");
        let path = dir.socket("bar.sock");
        let listener = bind_at(&path).unwrap();
        send_line_at(&path, BarCommand::LauncherToggle.as_str()).unwrap();

        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        assert_eq!(BarCommand::parse(&line), Some(BarCommand::LauncherToggle));
    }

    #[test]
    fn sending_with_no_listener_says_so() {
        let dir = ScratchDir::new("nolistener");
        let err = send_line_at(&dir.socket("bar.sock"), "launcher-toggle").unwrap_err();
        assert!(
            matches!(err, ControlError::NotListening { .. }),
            "expected NotListening, got {err:?}"
        );
    }

    #[test]
    fn a_socket_file_left_by_a_dead_daemon_is_replaced() {
        let dir = ScratchDir::new("stale");
        let path = dir.socket("bar.sock");
        // A plain file standing where the socket goes is exactly what a
        // crashed daemon leaves behind: bind must clear it, not fail forever.
        std::fs::write(&path, b"").unwrap();

        let listener = bind_at(&path).unwrap();
        send_line_at(&path, "launcher-toggle").unwrap();
        assert!(listener.accept().is_ok());
    }

    #[test]
    fn a_live_daemon_is_not_evicted_by_a_second_bind() {
        let dir = ScratchDir::new("conflict");
        let path = dir.socket("bar.sock");
        let _first = bind_at(&path).unwrap();
        let err = bind_at(&path).unwrap_err();
        assert!(
            matches!(err, ControlError::AlreadyBound(_)),
            "expected AlreadyBound, got {err:?}"
        );
    }
}
