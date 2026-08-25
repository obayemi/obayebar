//! The `--next` / `--reload` channel: a unix socket in the runtime dir.
//!
//! A socket rather than D-Bus because this binary otherwise needs neither an
//! async runtime nor a bus connection, and "rotate now" is a single line of
//! text. The socket lives in `$XDG_RUNTIME_DIR`, which is mode 700 and cleared
//! at logout, so a stale one cannot outlive the session that owns it.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

/// One command a running daemon accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Rotate every monitor now, and restart the interval.
    Next,
    /// Re-scan the wallpaper directory, keeping what is on screen if possible.
    Reload,
}

impl Command {
    /// The wire form. Deliberately plain text so `socat` can drive it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Reload => "reload",
        }
    }

    /// Parse a line from the socket.
    pub fn parse(line: &str) -> Option<Self> {
        match line.trim() {
            "next" => Some(Self::Next),
            "reload" => Some(Self::Reload),
            _ => None,
        }
    }
}

/// Where the control socket lives, or `None` without a runtime dir.
#[must_use]
pub fn socket_path() -> Option<PathBuf> {
    obayebar_core::xdg::runtime_dir().map(|d| d.join("wallpaper.sock"))
}

/// Send `command` to a running daemon.
///
/// # Errors
///
/// Returns a message naming what went wrong — most usefully, that nothing is
/// listening, which means the daemon is not running.
pub fn send(command: Command) -> Result<(), String> {
    let path = socket_path().ok_or_else(|| "XDG_RUNTIME_DIR is unset".to_string())?;
    let mut stream = UnixStream::connect(&path).map_err(|e| {
        format!(
            "no daemon listening on {} ({e}); is obayebar-wallpaper running?",
            path.display()
        )
    })?;
    writeln!(stream, "{}", command.as_str()).map_err(|e| format!("sending the command: {e}"))
}

/// Bind the control socket, replacing a stale one.
///
/// # Errors
///
/// Returns a message when there is no runtime dir or the socket cannot be
/// bound. Callers should treat that as "no control channel" and carry on
/// rendering rather than refusing to start.
pub fn listen() -> Result<UnixListener, String> {
    // Through the shared helper so the directory ends up mode 700 whichever
    // binary creates it first — it also holds the generated lock config, which
    // names every wallpaper path.
    obayebar_core::xdg::runtime_dir_create()
        .map_err(|e| format!("preparing the runtime directory: {e}"))?;
    let path = socket_path().ok_or_else(|| "XDG_RUNTIME_DIR is unset".to_string())?;
    // A socket file left by a crashed daemon would make bind fail with
    // EADDRINUSE forever. Probing it first distinguishes "stale" from "a
    // daemon is already running", which is a real error worth reporting.
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            return Err(format!(
                "another obayebar-wallpaper is already listening on {}",
                path.display()
            ));
        }
        let _ = std::fs::remove_file(&path);
    }
    let listener =
        UnixListener::bind(&path).map_err(|e| format!("binding {}: {e}", path.display()))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("setting the socket non-blocking: {e}"))?;
    Ok(listener)
}

/// Drain every pending command without blocking.
pub fn poll(listener: &UnixListener) -> Vec<Command> {
    let mut out = Vec::new();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let mut line = String::new();
                if BufReader::new(stream).read_line(&mut line).is_ok() {
                    match Command::parse(&line) {
                        Some(cmd) => out.push(cmd),
                        None => log::warn!("wallpaper: ignoring command {:?}", line.trim()),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return out,
            Err(e) => {
                log::warn!("wallpaper: control socket error ({e})");
                return out;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn commands_round_trip_through_the_wire_form() {
        for cmd in [Command::Next, Command::Reload] {
            assert_eq!(Command::parse(cmd.as_str()), Some(cmd));
        }
    }

    #[test]
    fn trailing_whitespace_is_tolerated() {
        // The client writes a newline; a human using socat may add more.
        assert_eq!(Command::parse("next\n"), Some(Command::Next));
        assert_eq!(Command::parse("  reload  \r\n"), Some(Command::Reload));
    }

    #[test]
    fn unknown_commands_are_rejected_rather_than_guessed() {
        for bad in ["", "NEXT", "nex", "rotate", "next please"] {
            assert_eq!(Command::parse(bad), None, "{bad:?}");
        }
    }
}
