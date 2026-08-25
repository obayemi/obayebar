//! `obayebar-wallpaper` — a random wallpaper per monitor, rotated on a timer.
//!
//! Replaces the `hyprwallp` fish script. See the Wallpapers section of the
//! README for why this renders directly rather than driving hyprpaper.

mod cli;
mod control;
mod decode;
mod render;
mod rotation;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::generic::Generic;
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::calloop::{EventLoop, Interest, Mode, PostAction};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;

use obayebar_core::config::Config;
use obayebar_core::wallpaper::{self, plan, state};

use rotation::Trigger;

fn main() {
    // Default to info, matching the bar; RUST_LOG still overrides.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

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
/// The assignment is always pushed, even when it matches what the state file
/// already said: on a fresh process the surfaces have nothing drawn on them.
/// The renderer skips a redraw when the picture and size are unchanged, so
/// repeating an assignment is cheap. Only the state *file* is conditional.
fn apply(
    renderer: &mut render::Renderer,
    settings: &Settings,
    current: &mut state::State,
    trigger: Trigger,
) {
    let monitors = match obayebar_core::hypr::monitors_blocking() {
        Ok(monitors) => monitors,
        Err(e) => {
            log::warn!("wallpaper: cannot read the monitor list ({e})");
            return;
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
            if assignment.state != *current {
                *current = assignment.state;
                if let Some(path) = settings.state_path.as_ref() {
                    state::save(path, current);
                }
            }
        }
        Err(reason) => log::warn!("wallpaper: nothing to do ({reason:?})"),
    }
}

fn run(args: &cli::Args) -> Result<(), String> {
    let settings = resolve(args)?;
    let mut current = load_state(settings.state_path.as_ref());

    let (mut renderer, connection, mut queue) =
        render::Renderer::new().map_err(|e| e.to_string())?;

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

    let known = renderer.output_names();

    // calloop rather than a loop around `blocking_dispatch`. The compositor
    // sends nothing at all to a process showing a static picture, so a blocking
    // dispatch parks indefinitely — and the control socket and the rotation
    // timer never get a turn. Here all three wait on one poll.
    // `WaylandSource` ties the loop's data type to the queue's, so the loop
    // carries the `Renderer` and everything else rides along in a shared cell.
    // The loop is single-threaded, so this is only about reaching the same
    // values from three callbacks.
    let mut event_loop: EventLoop<render::Renderer> =
        EventLoop::try_new().map_err(|e| format!("creating the event loop: {e}"))?;
    let handle = event_loop.handle();

    WaylandSource::new(connection, queue)
        .insert(handle.clone())
        .map_err(|e| format!("watching the wayland connection: {e}"))?;

    let context = Rc::new(RefCell::new(Context {
        settings,
        current,
        known,
    }));

    if let Some(listener) = listener {
        let context = Rc::clone(&context);
        handle
            .insert_source(
                Generic::new(listener, Interest::READ, Mode::Level),
                move |_, listener, renderer: &mut render::Renderer| {
                    for command in control::poll(listener) {
                        let trigger = match command {
                            control::Command::Next => Trigger::Rotate,
                            control::Command::Reload => Trigger::Restore,
                        };
                        context.borrow_mut().pass(renderer, trigger);
                    }
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|e| format!("watching the control socket: {e}"))?;
    }

    if let Some(every) = context.borrow().settings.interval {
        let context = Rc::clone(&context);
        handle
            .insert_source(
                Timer::from_duration(every),
                move |_, (), renderer: &mut render::Renderer| {
                    context.borrow_mut().pass(renderer, Trigger::Rotate);
                    TimeoutAction::ToDuration(every)
                },
            )
            .map_err(|e| format!("arming the rotation timer: {e}"))?;
    }

    // No timeout: every source that matters — the wayland connection, the
    // control socket, the rotation timer — is registered, so the loop can sleep
    // until one of them speaks. Hotplug is a flag the output handlers set, so
    // it rides in on the wayland event that caused it rather than needing a
    // poll.
    let hotplug = Rc::clone(&context);
    event_loop
        .run(None, &mut renderer, move |renderer| {
            if renderer.take_outputs_changed() {
                hotplug.borrow_mut().check_outputs(renderer);
            }
        })
        .map_err(|e| format!("running the event loop: {e}"))
}

/// The non-wayland half of the loop's state.
struct Context {
    settings: Settings,
    current: state::State,
    /// Output names as of the last check, for spotting hotplug.
    known: Vec<String>,
}

impl Context {
    fn pass(&mut self, renderer: &mut render::Renderer, trigger: Trigger) {
        apply(renderer, &self.settings, &mut self.current, trigger);
    }

    /// The output set changing is the hotplug signal. Comparing names is
    /// enough: the renderer creates and destroys the surfaces itself, and this
    /// only decides whether a *selection* pass is warranted.
    fn check_outputs(&mut self, renderer: &mut render::Renderer) {
        let names = renderer.output_names();
        if names != self.known {
            log::info!("wallpaper: outputs changed ({} now)", names.len());
            self.known = names;
            self.pass(renderer, Trigger::Hotplug);
        }
    }
}
