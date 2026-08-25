//! The `--next` / `--reload` channel: a unix socket in the runtime dir.
//!
//! The socket mechanics live in `obayebar_core::control`, shared with the bar's
//! control channel; what stays here is this daemon's own command vocabulary.

use std::io::{BufRead as _, BufReader};
use std::os::unix::net::UnixListener;

use obayebar_core::control::{self, WALLPAPER_SOCKET};

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

/// Send `command` to a running daemon.
///
/// # Errors
///
/// Returns a message naming what went wrong — most usefully, that nothing is
/// listening, which means the daemon is not running.
pub fn send(command: Command) -> Result<(), String> {
    control::send_line(WALLPAPER_SOCKET, command.as_str()).map_err(|e| match e {
        control::ControlError::NotListening { path, source } => format!(
            "no daemon listening on {} ({source}); is obayebar-wallpaper running?",
            path.display()
        ),
        other => other.to_string(),
    })
}

/// Bind the control socket, replacing a stale one.
///
/// # Errors
///
/// Returns a message when there is no runtime dir or the socket cannot be
/// bound. Callers should treat that as "no control channel" and carry on
/// rendering rather than refusing to start.
pub fn listen() -> Result<UnixListener, String> {
    control::bind(WALLPAPER_SOCKET).map_err(|e| e.to_string())
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
                        Some(cmd) => {
                            log::info!(
                                "wallpaper: received {} over the control socket",
                                cmd.as_str()
                            );
                            out.push(cmd);
                        }
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
