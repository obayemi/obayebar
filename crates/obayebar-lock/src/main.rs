//! `obayebar-lock` — lock the session with hyprlock, showing the wallpaper the
//! desktop is showing.
//!
//! Replaces the `hyprrandlock` fish script. A one-shot: enumerate the monitors,
//! read what the wallpaper daemon last chose, generate a hyprlock config with
//! one background per screen, run hyprlock, exit with its result. No async
//! runtime, no GUI toolkit — this stands between the user and a locked screen,
//! so it should start instantly and have very little that can go wrong.
//!
//! It is not a native `ext-session-lock` client on purpose. The available iced
//! binding discards the protocol's `locked` and `finished` events and calls
//! `unlock_and_destroy` unconditionally, whose resulting protocol error lands
//! on an `.expect()` — and a lock client that dies without unlocking leaves the
//! compositor locked with no way in. Driving hyprlock keeps that failure mode
//! out of the picture entirely.

mod cli;
mod compose;
mod spawn;

use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use obayebar_core::config::Config;
use obayebar_core::wallpaper::state;

fn main() {
    env_logger::init();

    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(cli::Error::Usage(message)) => {
            eprintln!("obayebar-lock: {message}");
            eprintln!("{}", cli::USAGE);
            std::process::exit(2);
        }
        Err(cli::Error::Exit(text)) => {
            println!("{text}");
            std::process::exit(0);
        }
    };

    std::process::exit(run(&args));
}

/// Read the base hyprlock config.
///
/// Never a vendored template. The live file carries things a copy in this
/// repository would not — on this machine an `auth { fingerprint { … } }`
/// block — so shipping our own would silently disable fingerprint unlock.
fn read_base(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| {
        format!(
            "cannot read the hyprlock config at {} ({e}); \
             point --config or [lock].config at one",
            path.display()
        )
    })
}

/// Write the generated config somewhere only this user can read it.
///
/// It names every wallpaper path, and the directory it goes in is mode 700, so
/// the file gets 0600 to match rather than inheriting a default umask.
fn write_config(path: &Path, contents: &str) -> Result<(), String> {
    obayebar_core::xdg::runtime_dir_create()
        .map_err(|e| format!("preparing the runtime directory ({e})"))?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("writing {} ({e})", path.display()))?;
    file.write_all(contents.as_bytes())
        .map_err(|e| format!("writing {} ({e})", path.display()))
}

fn run(args: &cli::Args) -> i32 {
    let config = Config::load();

    let base_path = args
        .config
        .clone()
        .unwrap_or_else(|| config.lock.config_path());
    let base = match read_base(&base_path) {
        Ok(base) => base,
        Err(message) => {
            eprintln!("obayebar-lock: {message}");
            // An explicitly named file that does not exist is a usage error;
            // a missing default is a configuration problem.
            return if args.config.is_some() { 2 } else { 1 };
        }
    };

    // A monitor query that fails is not fatal. Losing the per-monitor
    // backgrounds means falling back to the base config's own — a worse-looking
    // lock screen, which is far better than no lock screen.
    let monitors = match obayebar_core::hypr::monitors_blocking() {
        Ok(monitors) => monitors,
        Err(e) => {
            log::warn!("lock: cannot read the monitor list ({e}), using the base config alone");
            Vec::new()
        }
    };

    let state = if args.no_wallpaper {
        None
    } else {
        let path = args.state.clone().or_else(state::default_path);
        path.as_deref().and_then(state::load)
    };
    if state.is_none() && !args.no_wallpaper {
        log::info!("lock: no wallpaper state, using the base config's background");
    }

    let blur = args.blur.map_or_else(
        || config.lock.blur(),
        |(passes, size)| obayebar_core::wallpaper::hyprlock::Blur { passes, size },
    );

    let composed = compose::compose(&base, &monitors, state.as_ref(), blur);

    for skipped in &composed.skipped {
        log::info!(
            "lock: {} has no background ({})",
            skipped.monitor,
            skipped.reason
        );
    }
    for rejected in &composed.rendered.rejected {
        log::warn!(
            "lock: dropped {} for {} ({})",
            rejected.path.display(),
            rejected.monitor,
            rejected.reason
        );
    }
    if composed.missing_fallback {
        log::warn!(
            "lock: {} has no background block without a monitor; a screen that wakes \
             while locked will render transparent",
            base_path.display()
        );
    }

    if args.print {
        print!("{}", composed.rendered.config);
        return 0;
    }

    if args.check {
        let problems = composed.rendered.rejected.len();
        if problems > 0 || composed.missing_fallback {
            eprintln!(
                "obayebar-lock: {problems} unusable path(s){}",
                if composed.missing_fallback {
                    ", and no monitor-less background in the base config"
                } else {
                    ""
                }
            );
            return 1;
        }
        println!("obayebar-lock: config is usable");
        return 0;
    }

    let Some(out_path) = compose::output_path() else {
        // Without a runtime dir there is nowhere private to put the generated
        // config. Lock with the user's own file rather than refusing: a
        // plainer lock screen beats an unlocked machine.
        log::warn!("lock: XDG_RUNTIME_DIR is unset, locking with the base config unchanged");
        return finish(&base_path, args);
    };

    if let Err(message) = write_config(&out_path, &composed.rendered.config) {
        log::warn!("lock: {message}, locking with the base config unchanged");
        return finish(&base_path, args);
    }

    finish(&out_path, args)
}

/// Run hyprlock against `config` and turn the result into an exit code.
fn finish(config: &Path, args: &cli::Args) -> i32 {
    // The one line that says the lock was actually asked for, as opposed to
    // the process having started and fallen over somewhere. Whatever invoked
    // this — a keybind, hypridle, a suspend hook — leaves no trace of its own.
    log::info!(
        "lock: locking with {}{}",
        config.display(),
        if args.no_scope {
            " (no systemd scope)"
        } else {
            ""
        }
    );
    let outcome = spawn::lock(
        config,
        spawn::Options {
            scope: !args.no_scope,
            detach: args.detach,
            grace: args.grace,
        },
    );
    let (code, message) = spawn::report(&outcome);
    if let Some(message) = message {
        eprintln!("obayebar-lock: {message}");
    }
    code
}
