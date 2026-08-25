//! `obayebar-wallpaper` — a random wallpaper per monitor, rotated on a timer.
//!
//! Replaces the `hyprwallp` fish script. See the Wallpapers section of the
//! README for why this renders directly rather than driving hyprpaper.

mod cli;
mod control;
mod decode;
mod render;
mod rotation;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use obayebar_core::config::Config;
use obayebar_core::wallpaper::{self, plan, state};

use rotation::Trigger;

/// How often the event loop wakes to check the timer and the control socket
/// when there is nothing else to do.
///
/// The wayland connection is drained on every pass, so this only bounds how
/// late a rotation or a `--next` can be, not how responsive the surfaces are.
const TICK: Duration = Duration::from_millis(250);

fn main() {
    env_logger::init();

    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(cli::Error::Usage(message)) => {
            eprintln!("obayebar-wallpaper: {message}");
            eprintln!("{}", cli::USAGE);
            std::process::exit(2);
        }
        Err(cli::Error::Exit(text)) => {
            println!("{text}");
            std::process::exit(0);
        }
    };

    if let Some(command) = args.command {
        match control::send(command) {
            Ok(()) => return,
            Err(e) => {
                eprintln!("obayebar-wallpaper: {e}");
                std::process::exit(1);
            }
        }
    }

    if let Err(e) = run(&args) {
        eprintln!("obayebar-wallpaper: {e}");
        std::process::exit(1);
    }
}

/// Everything the process needs, after config and flags are resolved.
struct Settings {
    directory: PathBuf,
    interval: Option<Duration>,
    state_path: Option<PathBuf>,
}

fn resolve(args: &cli::Args) -> Result<Settings, String> {
    let config = Config::load();

    let directory = args
        .directory
        .clone()
        .unwrap_or_else(|| config.wallpaper.directory());

    let spec = args
        .interval
        .clone()
        .unwrap_or_else(|| config.wallpaper.interval().to_string());
    let interval = plan::parse_interval(&spec).map_err(|e| e.to_string())?;

    Ok(Settings {
        directory,
        interval,
        state_path: state::default_path(),
    })
}

/// A seed for the shuffle.
///
/// The wall clock rather than a random source: the only requirement is that two
/// runs do not deal the same order, and this keeps the binary free of an RNG
/// dependency. A fixed seed would make every login identical.
fn seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        // Truncating to the low 64 bits is the point: it is a seed, and the
        // nanosecond counter has far more range than it needs.
        .map_or(0, |d| {
            u64::try_from(d.as_nanos() & u128::from(u64::MAX)).unwrap_or(0)
        })
}

fn load_state(path: Option<&PathBuf>) -> state::State {
    path.and_then(|p| state::load(p)).unwrap_or_default()
}

/// Run one selection pass and hand the result to the renderer.
///
/// Returns whether anything changed, so the caller knows if the state file is
/// worth rewriting.
fn apply(
    renderer: &mut render::Renderer,
    settings: &Settings,
    current: &mut state::State,
    trigger: Trigger,
) -> bool {
    let monitors = match obayebar_core::hypr::monitors_blocking() {
        Ok(monitors) => monitors,
        Err(e) => {
            log::warn!("wallpaper: cannot read the monitor list ({e})");
            return false;
        }
    };
    let available = wallpaper::discover(&settings.directory);

    match rotation::decide(&monitors, &available, current, trigger, seed()) {
        Ok(assignment) => {
            for (port, path) in &assignment.by_port {
                log::info!("wallpaper: {port} -> {}", path.display());
                renderer.assign(port, path.clone());
            }
            renderer.refresh();
            *current = assignment.state;
            if let Some(path) = settings.state_path.as_ref() {
                state::save(path, current);
            }
            true
        }
        Err(rotation::Idle::AlreadySettled) => false,
        Err(reason) => {
            log::warn!("wallpaper: nothing to do ({reason:?})");
            false
        }
    }
}

fn run(args: &cli::Args) -> Result<(), String> {
    let settings = resolve(args)?;
    let mut current = load_state(settings.state_path.as_ref());

    let (mut renderer, mut queue) = render::Renderer::new().map_err(|e| e.to_string())?;

    // One round trip so the compositor tells us about its outputs before the
    // first selection pass — otherwise there is nothing to assign to.
    queue
        .roundtrip(&mut renderer)
        .map_err(|e| format!("initial wayland roundtrip: {e}"))?;

    apply(&mut renderer, &settings, &mut current, Trigger::Restore);

    if args.once {
        // Flush the surfaces we just drew, then leave. Used for testing and
        // for a one-shot "set it and exit" invocation.
        queue
            .roundtrip(&mut renderer)
            .map_err(|e| format!("final wayland roundtrip: {e}"))?;
        return Ok(());
    }

    // A missing control socket is not fatal: rendering is the job, and being
    // unable to accept `--next` is a degraded mode, not a failure.
    let listener = match control::listen() {
        Ok(listener) => Some(listener),
        Err(e) => {
            log::warn!("wallpaper: no control socket ({e})");
            None
        }
    };

    match settings.interval {
        Some(every) => log::info!("wallpaper: rotating every {}s", every.as_secs()),
        None => log::info!("wallpaper: rotation disabled"),
    }

    let mut known = renderer.output_names();
    let mut deadline = settings.interval.map(|every| Instant::now() + every);

    while !renderer.exit {
        queue
            .blocking_dispatch(&mut renderer)
            .map_err(|e| format!("wayland dispatch: {e}"))?;

        // The output set changing is the hotplug signal. Comparing names is
        // enough: the renderer creates and destroys surfaces itself, and this
        // only decides whether a *selection* pass is warranted.
        let names = renderer.output_names();
        if names != known {
            log::info!("wallpaper: outputs changed ({} now)", names.len());
            known = names;
            apply(&mut renderer, &settings, &mut current, Trigger::Hotplug);
        }

        if let Some(listener) = listener.as_ref() {
            for command in control::poll(listener) {
                match command {
                    control::Command::Next => {
                        apply(&mut renderer, &settings, &mut current, Trigger::Rotate);
                        deadline = settings.interval.map(|every| Instant::now() + every);
                    }
                    control::Command::Reload => {
                        apply(&mut renderer, &settings, &mut current, Trigger::Restore);
                    }
                }
            }
        }

        if let (Some(at), Some(every)) = (deadline, settings.interval) {
            if Instant::now() >= at {
                apply(&mut renderer, &settings, &mut current, Trigger::Rotate);
                deadline = Some(Instant::now() + every);
            }
        }

        // `blocking_dispatch` returns as soon as the compositor says anything,
        // so on a quiet system this is what bounds timer latency.
        std::thread::sleep(TICK);
    }

    Ok(())
}
