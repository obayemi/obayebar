//! `obayebar-wallpaper` — a random wallpaper per monitor, rotated on a timer.
//!
//! Replaces the `hyprwallp` fish script. See the Wallpapers section of the
//! README for why this renders directly rather than driving hyprpaper.

mod cli;
mod control;
mod decode;
mod render;
mod rotation;
mod settle;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::calloop::generic::Generic;
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::calloop::{
    EventLoop, Interest, LoopHandle, Mode, PostAction,
};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;

use obayebar_core::config::Config;
use obayebar_core::hypr;
use obayebar_core::wallpaper::{self, plan, state};

use rotation::Trigger;
use settle::Pass;

/// How long to wait before opening the Hyprland event socket again.
///
/// Matches the bar's retry on the same socket: a compositor that is restarting
/// comes back quickly, and one that is gone for good must not be hammered.
const HYPR_RETRY: Duration = Duration::from_secs(2);

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
///
/// Reports whether every monitor Hyprland lists came away with its picture, so
/// a screen that has been announced but has no surface yet is chased rather
/// than left bare until the next rotation.
fn apply(
    renderer: &mut render::Renderer,
    settings: &Settings,
    current: &mut state::State,
    trigger: Trigger,
) -> Pass {
    let monitors = match hypr::monitors_blocking() {
        Ok(monitors) => monitors,
        Err(e) => {
            // Unknown, not empty: worth asking again rather than acting on it.
            log::warn!("wallpaper: cannot read the monitor list ({e})");
            return Pass::Waiting;
        }
    };
    let available = wallpaper::discover(&settings.directory);

    match rotation::decide(&monitors, &available, current, trigger, seed()) {
        Ok(assignment) => {
            let mut waiting: Vec<&str> = Vec::new();
            for (port, path) in &assignment.by_port {
                if renderer.assign(port, path.clone()) {
                    log::info!("wallpaper: {port} -> {}", path.display());
                } else {
                    waiting.push(port);
                }
            }
            renderer.refresh();
            if assignment.state != *current {
                *current = assignment.state;
                if let Some(path) = settings.state_path.as_ref() {
                    state::save(path, current);
                }
            }
            if waiting.is_empty() {
                Pass::Complete
            } else {
                log::info!("wallpaper: {} has no surface yet", waiting.join(", "));
                Pass::Waiting
            }
        }
        Err(reason) => {
            log::warn!("wallpaper: nothing to do ({reason:?})");
            // An empty monitor list means we asked at a bad moment, which is a
            // different thing from a machine whose only screens are mirrors.
            if monitors.is_empty() {
                Pass::Waiting
            } else {
                Pass::Complete
            }
        }
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

    let startup = apply(&mut renderer, &settings, &mut current, Trigger::Restore);

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
    let mut event_loop: EventLoop<'static, render::Renderer> =
        EventLoop::try_new().map_err(|e| format!("creating the event loop: {e}"))?;
    let handle = event_loop.handle();

    WaylandSource::new(connection, queue)
        .insert(handle.clone())
        .map_err(|e| format!("watching the wayland connection: {e}"))?;

    let context = Rc::new(RefCell::new(Context {
        settings,
        current,
        known,
        settle: settle::Settle::default(),
    }));

    if let Some(listener) = listener {
        let controlled = Rc::clone(&context);
        let scheduler = handle.clone();
        handle
            .insert_source(
                Generic::new(listener, Interest::READ, Mode::Level),
                move |_, listener, renderer: &mut render::Renderer| {
                    for command in control::poll(listener) {
                        let trigger = match command {
                            control::Command::Next => Trigger::Rotate,
                            control::Command::Reload => Trigger::Restore,
                        };
                        let pass = controlled.borrow_mut().pass(renderer, trigger);
                        chase(&scheduler, &controlled, pass);
                    }
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|e| format!("watching the control socket: {e}"))?;
    }

    if let Some(every) = context.borrow().settings.interval {
        let rotating = Rc::clone(&context);
        let scheduler = handle.clone();
        handle
            .insert_source(
                Timer::from_duration(every),
                move |_, (), renderer: &mut render::Renderer| {
                    let pass = rotating.borrow_mut().pass(renderer, Trigger::Rotate);
                    chase(&scheduler, &rotating, pass);
                    TimeoutAction::ToDuration(every)
                },
            )
            .map_err(|e| format!("arming the rotation timer: {e}"))?;
    }

    // The second half of the hotplug signal, and the same one the bar watches.
    watch_monitor_events(&handle, &context);

    // The startup pass ran before the loop existed, so its follow-up could not
    // be queued then. A cold start is exactly when a monitor is most likely to
    // be named before its surface exists.
    chase(&handle, &context, startup);

    // No timeout: every source that matters — the wayland connection, the
    // control socket, the Hyprland event socket, the rotation timer — is
    // registered, so the loop can sleep until one of them speaks. A new output
    // is a flag the output handlers set, so it rides in on the wayland event
    // that caused it rather than needing a poll.
    let hotplug = Rc::clone(&context);
    let hotplug_handle = handle.clone();
    event_loop
        .run(None, &mut renderer, move |renderer| {
            if renderer.take_outputs_changed() {
                if let Some(pass) = hotplug.borrow_mut().check_outputs(renderer) {
                    chase(&hotplug_handle, &hotplug, pass);
                }
            }
        })
        .map_err(|e| format!("running the event loop: {e}"))
}

/// Watch Hyprland's event socket for changes to the monitor set.
///
/// The wayland side notices an output appearing, and on its own that was the
/// whole hotplug signal here — but it is only half of one. The compositor
/// creates the `wl_output` and Hyprland answers `j/monitors` independently,
/// with no ordering between them, so a pass driven by the wayland half can run
/// before the monitor is listed and then nothing runs again. The bar watches
/// this socket for the same reason. Watching both means whichever half lands
/// last still triggers a pass, and `is_monitor_event` is the bar's list, not a
/// second copy of it.
fn watch_monitor_events(
    handle: &LoopHandle<'static, render::Renderer>,
    context: &Rc<RefCell<Context>>,
) {
    let socket = match hypr::EventSocket::connect() {
        Ok(socket) => socket,
        Err(e) => {
            log::warn!("wallpaper: no hyprland event socket ({e}), retrying");
            watch_monitor_events_later(handle, context);
            return;
        }
    };

    let watcher = handle.clone();
    let watching = Rc::clone(context);
    let inserted = handle.insert_source(
        Generic::new(socket, Interest::READ, Mode::Level),
        move |_, socket, renderer: &mut render::Renderer| {
            let batch = socket.read_available();
            if batch.has_monitor_event() {
                log::info!("wallpaper: hyprland reports a change to the monitors");
                let pass = watching.borrow_mut().pass(renderer, Trigger::Hotplug);
                chase(&watcher, &watching, pass);
            }
            if batch.closed {
                log::warn!("wallpaper: the hyprland event socket closed, reconnecting");
                watch_monitor_events_later(&watcher, &watching);
                return Ok(PostAction::Remove);
            }
            Ok(PostAction::Continue)
        },
    );
    if let Err(e) = inserted {
        log::warn!("wallpaper: cannot watch the hyprland event socket ({e})");
        watch_monitor_events_later(handle, context);
    }
}

/// Try the event socket again shortly. Losing it degrades hotplug to the
/// wayland half alone, so this keeps trying rather than giving up.
fn watch_monitor_events_later(
    handle: &LoopHandle<'static, render::Renderer>,
    context: &Rc<RefCell<Context>>,
) {
    let retry = handle.clone();
    let retrying = Rc::clone(context);
    let armed = handle.insert_source(
        Timer::from_duration(HYPR_RETRY),
        move |_, (), _renderer: &mut render::Renderer| {
            watch_monitor_events(&retry, &retrying);
            TimeoutAction::Drop
        },
    );
    if let Err(e) = armed {
        log::error!(
            "wallpaper: cannot schedule the hyprland reconnect ({e}); hotplug now rests on wayland alone"
        );
    }
}

/// Queue another pass when the last one left a monitor without its picture.
fn chase(
    handle: &LoopHandle<'static, render::Renderer>,
    context: &Rc<RefCell<Context>>,
    pass: Pass,
) {
    let Some(delay) = context.borrow_mut().settle.schedule(pass, Instant::now()) else {
        return;
    };
    log::debug!("wallpaper: another pass in {}ms", delay.as_millis());

    let again = handle.clone();
    let chasing = Rc::clone(context);
    let armed = handle.insert_source(
        Timer::from_duration(delay),
        move |_, (), renderer: &mut render::Renderer| {
            let pass = chasing.borrow_mut().settle_pass(renderer);
            chase(&again, &chasing, pass);
            TimeoutAction::Drop
        },
    );
    if let Err(e) = armed {
        log::warn!("wallpaper: cannot schedule another pass ({e})");
        context.borrow_mut().settle.fired();
    }
}

/// The non-wayland half of the loop's state.
struct Context {
    settings: Settings,
    current: state::State,
    /// Output names as of the last check, for spotting hotplug.
    known: Vec<String>,
    /// The follow-up schedule for a pass that could not reach every monitor.
    settle: settle::Settle,
}

impl Context {
    fn pass(&mut self, renderer: &mut render::Renderer, trigger: Trigger) -> Pass {
        apply(renderer, &self.settings, &mut self.current, trigger)
    }

    /// The pass a scheduled follow-up runs. Clearing the flag first is what
    /// lets this one queue the next.
    fn settle_pass(&mut self, renderer: &mut render::Renderer) -> Pass {
        self.settle.fired();
        self.pass(renderer, Trigger::Hotplug)
    }

    /// The output set changing is one half of the hotplug signal. Comparing
    /// names is enough: the renderer creates and destroys the surfaces itself,
    /// and this only decides whether a *selection* pass is warranted.
    ///
    /// `None` when the set is unchanged — the wayland event was about
    /// something else, and reporting a pass that never ran would clear a
    /// follow-up that is still needed.
    fn check_outputs(&mut self, renderer: &mut render::Renderer) -> Option<Pass> {
        let names = renderer.output_names();
        if names == self.known {
            return None;
        }
        log::info!("wallpaper: outputs changed ({} now)", names.len());
        self.known = names;
        Some(self.pass(renderer, Trigger::Hotplug))
    }
}
