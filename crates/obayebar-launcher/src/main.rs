//! Keybinding shim: ask the running bar to show its launcher.
//!
//! The launcher itself lives in the bar daemon, whose entry list is already
//! parsed and whose icons are already decoded, so showing it costs one frame.
//! This binary exists so an existing `obayebar-launcher` keybinding keeps
//! working — it connects to a socket, writes one word, and exits.

use obayebar_core::control::{self, BarCommand, BAR_SOCKET};

const USAGE: &str = "\
obayebar-launcher [OPTIONS]

Toggles the application launcher drawn by the running obayebar.

  -h, --help      Print this help
  -V, --version   Print version";

fn main() {
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            "-V" | "--version" => {
                println!("obayebar-launcher {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            other => {
                eprintln!("obayebar-launcher: unknown argument '{other}'");
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }

    if let Err(err) = control::send_line(BAR_SOCKET, BarCommand::LauncherToggle.as_str()) {
        // Naming the bar matters here: "connection refused" on its own reads
        // like a bug in this command, when what it means is that the process
        // that draws the launcher is not running.
        match err {
            control::ControlError::NotListening { path, source } => eprintln!(
                "obayebar-launcher: nothing listening on {} ({source}); is obayebar running?",
                path.display()
            ),
            other => eprintln!("obayebar-launcher: {other}"),
        }
        std::process::exit(1);
    }
}
