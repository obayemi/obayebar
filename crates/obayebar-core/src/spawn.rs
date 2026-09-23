//! The one way obayebar starts a program on the user's behalf.
//!
//! `systemctl --user show obayebar.service` reports `KillMode=control-group`:
//! a plain child of the bar shares the bar's cgroup and is killed with it. So
//! `systemctl --user restart obayebar` would close every window the launcher
//! had opened, and — for the lock screen — would *unlock the machine*. Every
//! program started for the user therefore goes into its own transient systemd
//! unit under a slice of its own — [`DEFAULT_SLICE`], or whatever `[spawn]
//! slice` names — which is a cgroup a bar restart cannot reach.
//!
//! The slice is the other half of the bargain. It is one handle on everything
//! the bar ever launched: `systemctl --user kill app-obayebar.slice` stops the
//! lot, and because systemd-oomd scores a slice as a whole, a session running
//! out of memory sheds launched applications before it sheds the session's own
//! services. [`Program::protected`] opts a unit out of being picked first,
//! which is what keeps a lock screen up while the rest of the slice burns.
//!
//! Without a systemd user manager there is no cgroup to escape, but there is
//! still a process group and a controlling terminal, so the fallbacks detach
//! anyway: `setsid -f` when it exists, otherwise a backgrounding `sh -c`. Both
//! reparent the program to init and exit at once, so neither leaves a zombie
//! behind in the bar.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

/// The slice every program obayebar launches is placed in, unless
/// [`use_slice`] says otherwise.
///
/// `app-` is the prefix systemd reserves for user applications, which is what
/// the stock `~/.config/systemd/user/app-.slice.d` drop-ins and oomd policies
/// key off; putting the bar's children anywhere else would take them out of
/// scope of a configuration the user already has.
pub const DEFAULT_SLICE: &str = "app-obayebar.slice";

/// The slice named by `[spawn] slice` in the config file, once a binary has
/// installed it.
static CONFIGURED_SLICE: OnceLock<String> = OnceLock::new();

/// Apply the `[spawn]` section of the config file.
///
/// Call once at startup, after the logger and before anything is launched: a
/// [`Program`] takes its slice when it is built. A section that names no slice
/// leaves [`DEFAULT_SLICE`] in place.
pub fn install(config: &crate::config::SpawnConfig) {
    if let Some(slice) = config.slice.as_deref() {
        use_slice(slice);
    }
}

/// Put every program started from here in `slice` rather than in
/// [`DEFAULT_SLICE`].
///
/// A name systemd would reject is ignored with a warning rather than being
/// passed on. `systemd-run` would refuse the whole request, and one mistyped
/// line in a config file should not leave the launcher unable to launch
/// anything at all.
fn use_slice(slice: &str) {
    if !valid_slice_name(slice) {
        log::warn!("spawn: {slice:?} is not a usable slice name, keeping {DEFAULT_SLICE}");
        return;
    }
    if let Err(existing) = CONFIGURED_SLICE.set(slice.to_string()) {
        if existing != slice {
            log::warn!("spawn: the slice is already {existing:?}, ignoring {slice:?}");
        }
    }
}

/// The slice a new [`Program`] goes in.
fn configured_slice() -> &'static str {
    CONFIGURED_SLICE.get().map_or(DEFAULT_SLICE, String::as_str)
}

/// Whether systemd would accept `name` as a slice unit.
///
/// A slice's name *is* its position in the tree: `app-obayebar.slice` is a
/// child of `app.slice`, so a dash is a separator and cannot be doubled or sit
/// at either end. The rest is the unit-name alphabet.
fn valid_slice_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".slice") else {
        return false;
    };
    // 255 is systemd's limit on a whole unit name, including the suffix.
    if stem.is_empty() || name.len() > 255 {
        return false;
    }
    if stem.starts_with('-') || stem.ends_with('-') || stem.contains("--") {
        return false;
    }
    stem.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '\\'))
}

/// Prefix of every transient unit name, so `systemctl --user list-units
/// 'obayebar-*'` shows exactly what the bar started.
const UNIT_PREFIX: &str = "obayebar";

/// Session variables a transient *service* does not inherit.
///
/// A scope keeps our environment; a service is started by the user manager and
/// gets the manager's, which is only as complete as whatever ran
/// `systemctl --user import-environment` at login. Forwarding these explicitly
/// is the difference between the launcher starting a GUI application and the
/// application failing to find a display.
const SESSION_ENV: [&str; 16] = [
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "XDG_SESSION_TYPE",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS",
    "DBUS_SESSION_BUS_ADDRESS",
    "SSH_AUTH_SOCK",
    "PATH",
    "TERMINAL",
    "XCURSOR_THEME",
    "XCURSOR_SIZE",
    "LANG",
    "GTK_THEME",
];

/// What a [`Program::singleton`] does when its unit name is already taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnCollision {
    /// Leave the running one alone and fail with [`Error::AlreadyRunning`].
    Refuse,
    /// Take the running one down and start in its place.
    ///
    /// For a program whose unit proves nothing: hyprlock has been seen to hang
    /// after the screen was unlocked, and a guard reading the unit alone would
    /// then refuse to lock the session ever again. Replacing is safe there
    /// because `ext-session-lock` keeps a session locked when its client dies,
    /// so no desktop shows through the swap.
    Replace,
}

/// What owns the program once it is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// A transient user *service*: the systemd user manager is its parent, so
    /// it survives not just a bar restart but the bar's death. Nothing can be
    /// waited on — this is fire-and-forget.
    #[default]
    Service,
    /// A transient *scope*: its own cgroup, but still this process's child, so
    /// a caller can wait for it to exit. For the lock screen, which is only
    /// useful if someone can tell when it was dismissed.
    Scope,
}

/// The wrapper that detaches the program.
///
/// Chosen once by [`runner`] and passed around explicitly so that argv
/// construction stays a pure function the tests can check without starting
/// anything.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Runner {
    /// `systemd-run --user`, the only one that gives a cgroup and a name.
    Systemd(PathBuf),
    /// `setsid -f`: a new session, and the direct child exits immediately.
    Setsid(PathBuf),
    /// `sh -c '"$0" "$@" &'`: the last resort, and the only one with no
    /// dependency beyond a POSIX shell.
    Shell,
}

/// Why a program did not start.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("nothing to run")]
    Empty,
    #[error("{unit} is already running")]
    AlreadyRunning { unit: String },
    #[error("{unit} is already running and would not stop")]
    NotReplaced { unit: String },
    #[error("starting {program}: {source}")]
    Io {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{wrapper} refused to start {program}: {reason}")]
    Refused {
        wrapper: String,
        program: String,
        reason: String,
    },
}

/// A started program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    /// The transient unit that owns it, when systemd took it. `None` means a
    /// fallback runner detached it and nothing is tracking it by name.
    pub unit: Option<String>,
}

/// A program to start, and how it should be isolated.
///
/// ```no_run
/// use obayebar_core::spawn::Program;
/// Program::new("xdg-open").arg("https://example.invalid").tag("browser").spawn()?;
/// # Ok::<(), obayebar_core::spawn::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Program {
    argv: Vec<OsString>,
    tag: String,
    mode: Mode,
    /// Taken from [`configured_slice`] when the program is built, so a late
    /// [`use_slice`] cannot move a program that is already on its way out.
    slice: String,
    /// Fixed unit name, and what to do when that name is taken. `None` for a
    /// program that gets a fresh name every time and cannot collide.
    singleton: Option<OnCollision>,
    /// `ManagedOOMPreference=avoid`: oomd kills the rest of the slice first.
    protected: bool,
}

impl Program {
    /// Start building a program. `program` is argv[0].
    #[must_use]
    pub fn new(program: impl Into<OsString>) -> Self {
        Self::from_argv([program.into()])
    }

    /// Build from a whole argv. The first element is the program.
    #[must_use]
    pub fn from_argv<I, S>(argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            argv: argv.into_iter().map(Into::into).collect(),
            tag: "app".to_string(),
            mode: Mode::default(),
            slice: configured_slice().to_string(),
            singleton: None,
            protected: false,
        }
    }

    /// Put this one program in `slice` rather than in whatever the config
    /// chose. For a caller that has a reason to separate it from the rest.
    #[must_use]
    pub fn slice(mut self, slice: impl Into<String>) -> Self {
        self.slice = slice.into();
        self
    }

    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.argv.push(arg.into());
        self
    }

    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.argv.extend(args.into_iter().map(Into::into));
        self
    }

    /// Name this kind of program, e.g. `launcher` or `browser`. It becomes
    /// part of the unit name and of the unit description, so `systemctl
    /// --user status` says what the bar started something for.
    #[must_use]
    pub fn tag(mut self, tag: &str) -> Self {
        self.tag = sanitize_tag(tag);
        self
    }

    #[must_use]
    pub const fn mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    /// Give it a fixed unit name, so that a second start while the first runs
    /// meets `on_collision` rather than quietly becoming two programs.
    #[must_use]
    pub const fn singleton(mut self, on_collision: OnCollision) -> Self {
        self.singleton = Some(on_collision);
        self
    }

    /// What this program does about a unit of its name already running, if it
    /// is a [`singleton`](Self::singleton) at all.
    #[must_use]
    pub const fn on_collision(&self) -> Option<OnCollision> {
        self.singleton
    }

    /// Ask systemd-oomd to kill the rest of the slice before this. For the
    /// lock screen: a session shedding memory must not unlock itself.
    #[must_use]
    pub const fn protected(mut self) -> Self {
        self.protected = true;
        self
    }

    /// The unit name this program would get.
    ///
    /// Stable for a [`singleton`](Self::singleton), unique otherwise — a fresh
    /// one per call, so hold on to what [`spawn`](Self::spawn) reports rather
    /// than calling this twice and expecting the same answer.
    #[must_use]
    pub fn unit_name(&self) -> String {
        let suffix = match self.mode {
            Mode::Service => "service",
            Mode::Scope => "scope",
        };
        if self.singleton.is_some() {
            format!("{UNIT_PREFIX}-{}.{suffix}", self.tag)
        } else {
            format!(
                "{UNIT_PREFIX}-{}-{}-{}.{suffix}",
                self.tag,
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            )
        }
    }

    /// The command that would run, wrapper and all.
    ///
    /// For [`Mode::Scope`] this is what a caller waits on: the scope's program
    /// is the direct child, so `status()` on it returns when the program
    /// exits. Returns the unit name alongside, when there is one.
    ///
    /// # Errors
    ///
    /// [`Error::Empty`] for an empty argv, [`Error::AlreadyRunning`] when a
    /// singleton's unit is already up.
    pub fn command(&self) -> Result<(Command, Option<String>), Error> {
        let (runner, unit) = self.preflight()?;
        let command = self.command_with(&runner, unit.as_deref(), &env_snapshot());
        Ok((command, unit))
    }

    /// Pick the runner and claim a unit name, refusing before anything runs if
    /// there is nothing to run or the singleton is already up.
    fn preflight(&self) -> Result<(Runner, Option<String>), Error> {
        if self.argv.first().is_none_or(|p| p.is_empty()) {
            return Err(Error::Empty);
        }
        let runner = runner(self.mode);
        let unit = matches!(runner, Runner::Systemd(_)).then(|| self.unit_name());
        if let Some((unit, on_collision)) = unit.as_ref().zip(self.singleton) {
            claim(unit, on_collision)?;
            // A run that failed leaves the unit loaded, and `systemd-run
            // --unit=` then refuses with "unit is already loaded". Only a
            // singleton can collide with itself.
            let _ = systemctl(&["reset-failed", unit]);
        }
        Ok((runner, unit))
    }

    /// Start it and let go.
    ///
    /// The wrapper is waited on — it registers the unit and exits in a few
    /// milliseconds — so that a refusal is reported rather than lost, and so
    /// no zombie is left in the bar. The program itself is never waited on.
    ///
    /// For [`Mode::Service`] only. A scope's program *is* the direct child, so
    /// this would block until it exits; use [`command`](Self::command) there
    /// and wait on it deliberately.
    ///
    /// # Errors
    ///
    /// As [`command`](Self::command), plus [`Error::Io`] when the wrapper will
    /// not run and [`Error::Refused`] when it runs and rejects the request.
    pub fn spawn(&self) -> Result<Started, Error> {
        let (runner, unit) = self.preflight()?;
        match self.run(&runner, unit.as_deref()) {
            Ok(()) => Ok(Started { unit }),
            // systemd is there but would not take it: a property this version
            // does not know, a wedged manager, a name clash we did not catch.
            // Detaching it the old-fashioned way beats not starting it at all.
            Err(error) if matches!(runner, Runner::Systemd(_)) => {
                log::warn!("spawn: {error}; falling back to a plain detached start");
                self.run(&Runner::Shell, None)?;
                Ok(Started { unit: None })
            }
            Err(error) => Err(error),
        }
    }

    /// Run the wrapper to completion. Every runner exits as soon as the
    /// program is handed off, so this blocks for milliseconds at most.
    fn run(&self, runner: &Runner, unit: Option<&str>) -> Result<(), Error> {
        let output = self
            .command_with(runner, unit, &env_snapshot())
            .output()
            .map_err(|source| Error::Io {
                program: self.program_name(),
                source,
            })?;
        if output.status.success() {
            return Ok(());
        }
        Err(Error::Refused {
            wrapper: match runner {
                Runner::Systemd(_) => "systemd-run".to_string(),
                Runner::Setsid(_) => "setsid".to_string(),
                Runner::Shell => "sh".to_string(),
            },
            program: self.program_name(),
            reason: {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stderr = stderr.trim();
                if stderr.is_empty() {
                    format!("exit {}", output.status)
                } else {
                    stderr.to_string()
                }
            },
        })
    }

    fn program_name(&self) -> String {
        self.argv
            .first()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Build the command for a given runner.
    ///
    /// Pure but for the [`Stdio`] handles: everything that varies — which
    /// wrapper, which unit name, which environment — is an argument, so the
    /// tests below check the exact argv without starting a process.
    fn command_with(
        &self,
        runner: &Runner,
        unit: Option<&str>,
        env: &[(String, OsString)],
    ) -> Command {
        let argv = self.wrapped_argv(runner, unit, env);
        let (program, rest) = argv
            .split_first()
            .map_or((OsString::new(), [].as_slice()), |(p, r)| (p.clone(), r));
        let mut command = Command::new(program);
        command.args(rest);
        if self.mode == Mode::Service {
            // `run` reads stderr to report why systemd-run said no. A scope is
            // handed to the caller instead, and piping a stream nobody reads
            // would deadlock the program once its output filled the pipe.
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
        }
        command
    }

    /// The full argv, wrapper included.
    fn wrapped_argv(
        &self,
        runner: &Runner,
        unit: Option<&str>,
        env: &[(String, OsString)],
    ) -> Vec<OsString> {
        match runner {
            Runner::Systemd(path) => {
                let mut argv: Vec<OsString> = vec![path.into(), "--user".into(), "--quiet".into()];
                // Tear the unit down as soon as it is done, so a failed run
                // does not block the next one with the same name.
                argv.push("--collect".into());
                argv.push(format!("--slice={}", self.slice).into());
                argv.push(format!("--description=obayebar: {}", self.tag).into());
                // oomd only scores what it can measure.
                argv.push("--property=MemoryAccounting=yes".into());
                if self.protected {
                    argv.push("--property=ManagedOOMPreference=avoid".into());
                }
                if let Some(unit) = unit {
                    // The suffix is the unit type, which `--unit=` infers from
                    // `--scope`; passing it would name the unit "x.scope.scope".
                    let stem = unit.rsplit_once('.').map_or(unit, |(stem, _)| stem);
                    argv.push(format!("--unit={stem}").into());
                }
                match self.mode {
                    Mode::Scope => argv.push("--scope".into()),
                    Mode::Service => {
                        // Return once the unit exists rather than once the
                        // program is up: a launcher click should not wait on
                        // an application's startup.
                        argv.push("--no-block".into());
                        for (name, value) in env {
                            let mut arg = OsString::from(format!("--setenv={name}="));
                            arg.push(value);
                            argv.push(arg);
                        }
                    }
                }
                argv.push("--".into());
                argv.extend(self.argv.iter().cloned());
                argv
            }
            // A scope is the only thing that needs a wrapper to be waitable;
            // with no systemd there is no scope, so run the program itself and
            // let the caller wait on it.
            Runner::Setsid(_) | Runner::Shell if self.mode == Mode::Scope => self.argv.clone(),
            Runner::Setsid(path) => {
                let mut argv: Vec<OsString> = vec![path.into(), "-f".into(), "--".into()];
                argv.extend(self.argv.iter().cloned());
                argv
            }
            // `$0` is the program and `$@` the rest, so the shell never
            // re-splits an argument that contains a space. The `&` is the
            // whole point: sh exits at once and init adopts the program.
            Runner::Shell => {
                let mut argv: Vec<OsString> =
                    vec!["sh".into(), "-c".into(), r#""$0" "$@" &"#.into()];
                argv.extend(self.argv.iter().cloned());
                argv
            }
        }
    }
}

/// Serial number distinguishing two units started by the same bar.
static SERIAL: AtomicU32 = AtomicU32::new(0);

/// Unit names accept a narrow alphabet; anything else here would make
/// `systemd-run` reject the whole request, so fold it to a dash.
fn sanitize_tag(tag: &str) -> String {
    let folded: String = tag
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .take(48)
        .collect();
    let trimmed = folded.trim_matches('-');
    if trimmed.is_empty() {
        "app".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The best available way to detach, for this mode.
fn runner(mode: Mode) -> Runner {
    if systemd_user_manager_is_up() {
        if let Some(path) = find_in_path("systemd-run") {
            return Runner::Systemd(path);
        }
    }
    // A scope's only purpose is the cgroup. Detaching instead would take the
    // waitability away as well, leaving a caller that cannot tell when the
    // program it started finished.
    if mode == Mode::Scope {
        return Runner::Shell;
    }
    find_in_path("setsid").map_or(Runner::Shell, Runner::Setsid)
}

/// Whether there is a systemd user manager to talk to.
///
/// The private socket is the manager's own, so this answers the question that
/// matters — can `systemd-run --user` reach anything — rather than the weaker
/// "is systemd pid 1", which is true in a container with no user manager.
fn systemd_user_manager_is_up() -> bool {
    std::env::var_os("XDG_RUNTIME_DIR")
        .is_some_and(|dir| PathBuf::from(dir).join("systemd/private").exists())
}

/// Take the singleton's unit name, or explain why it cannot be had.
fn claim(unit: &str, on_collision: OnCollision) -> Result<(), Error> {
    if !unit_is_active(unit) {
        return Ok(());
    }
    let unit = unit.to_string();
    match on_collision {
        OnCollision::Refuse => Err(Error::AlreadyRunning { unit }),
        OnCollision::Replace if take_down(&unit) => Ok(()),
        OnCollision::Replace => Err(Error::NotReplaced { unit }),
    }
}

/// Take `unit` down now, reporting whether it is gone.
///
/// SIGKILL before the stop, because a program worth replacing is usually one
/// that stopped answering: `systemctl stop` alone would wait out
/// `DefaultTimeoutStopSec` — a minute and a half of an unlocked session, for
/// the lock screen — before systemd escalated on its own. The stop that
/// follows waits for its job and frees the name.
fn take_down(unit: &str) -> bool {
    let _ = systemctl(&["kill", "--signal=KILL", unit]);
    systemctl(&["stop", unit])
}

/// Whether `unit` is running.
fn unit_is_active(unit: &str) -> bool {
    systemctl(&["is-active", unit])
}

/// Run `systemctl --user` with nothing on the terminal, reporting success.
fn systemctl(args: &[&str]) -> bool {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The session variables worth forwarding that this process actually has.
fn env_snapshot() -> Vec<(String, OsString)> {
    SESSION_ENV
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| ((*name).to_string(), value)))
        .collect()
}

/// Resolve `name` against `$PATH`.
///
/// Used to pick a wrapper that exists rather than spawning a missing one and
/// reporting success, and by the launcher to honour `TryExec` and to find a
/// terminal emulator.
#[must_use]
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let path = PathBuf::from(name);
        return path.is_file().then_some(path);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn strings(argv: &[OsString]) -> Vec<String> {
        argv.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn systemd() -> Runner {
        Runner::Systemd(PathBuf::from("/usr/bin/systemd-run"))
    }

    #[test]
    fn a_service_is_named_placed_and_detached() {
        let program = Program::new("firefox").arg("--new-window").tag("browser");
        let argv =
            strings(&program.wrapped_argv(&systemd(), Some("obayebar-browser-7-0.service"), &[]));

        assert_eq!(
            argv.first().map(String::as_str),
            Some("/usr/bin/systemd-run")
        );
        assert!(argv.contains(&"--user".to_string()));
        assert!(
            argv.contains(&format!("--slice={DEFAULT_SLICE}")),
            "the slice is what makes one kill command reach every launched program"
        );
        assert!(
            argv.contains(&"--unit=obayebar-browser-7-0".to_string()),
            "the unit type suffix belongs to systemd-run, not to --unit="
        );
        assert!(
            argv.contains(&"--no-block".to_string()),
            "a launcher click must not wait on the application's startup"
        );
        // The program and its arguments survive, in order, after the `--`.
        let tail = argv.split(|a| a == "--").nth(1).unwrap().to_vec();
        assert_eq!(tail, vec!["firefox", "--new-window"]);
    }

    #[test]
    fn a_scope_keeps_the_program_as_our_child() {
        let program = Program::new("hyprlock").tag("lock").mode(Mode::Scope);
        let argv = strings(&program.wrapped_argv(&systemd(), Some("obayebar-lock.scope"), &[]));

        assert!(argv.contains(&"--scope".to_string()));
        assert!(
            !argv.contains(&"--no-block".to_string()),
            "returning before the scope exists would break waiting on it"
        );
        assert!(argv.contains(&"--unit=obayebar-lock".to_string()));
    }

    #[test]
    fn only_a_service_carries_the_session_environment() {
        let env = vec![("WAYLAND_DISPLAY".to_string(), OsString::from("wayland-1"))];
        let service = strings(&Program::new("foot").wrapped_argv(&systemd(), None, &env));
        assert!(service.contains(&"--setenv=WAYLAND_DISPLAY=wayland-1".to_string()));

        // A scope inherits our environment already; forwarding it again would
        // be noise in every `systemctl status`.
        let scope = strings(&Program::new("foot").mode(Mode::Scope).wrapped_argv(
            &systemd(),
            None,
            &env,
        ));
        assert!(!scope.iter().any(|a| a.starts_with("--setenv")));
    }

    #[test]
    fn a_protected_program_is_not_the_first_thing_oomd_kills() {
        let plain = strings(&Program::new("foot").wrapped_argv(&systemd(), None, &[]));
        assert!(!plain.iter().any(|a| a.contains("ManagedOOMPreference")));

        let guarded = strings(&Program::new("hyprlock").protected().wrapped_argv(
            &systemd(),
            None,
            &[],
        ));
        assert!(guarded.contains(&"--property=ManagedOOMPreference=avoid".to_string()));
        assert!(
            guarded.contains(&"--property=MemoryAccounting=yes".to_string()),
            "oomd cannot rank a unit it does not measure"
        );
    }

    #[test]
    fn setsid_detaches_without_re_splitting_arguments() {
        let argv = strings(
            &Program::new("sh")
                .args(["-c", "echo one two"])
                .wrapped_argv(&Runner::Setsid(PathBuf::from("/bin/setsid")), None, &[]),
        );
        assert_eq!(
            argv,
            vec!["/bin/setsid", "-f", "--", "sh", "-c", "echo one two"]
        );
    }

    #[test]
    fn the_shell_fallback_backgrounds_and_quotes() {
        let argv = strings(&Program::new("myapp").arg("a file.txt").wrapped_argv(
            &Runner::Shell,
            None,
            &[],
        ));
        assert_eq!(
            argv,
            vec!["sh", "-c", r#""$0" "$@" &"#, "myapp", "a file.txt"]
        );
    }

    #[test]
    fn a_scope_without_systemd_runs_the_program_itself() {
        // Wrapping it would detach it, and a lock screen nobody can wait on
        // is worse than one in the wrong cgroup on a machine that has none.
        let argv = strings(
            &Program::new("hyprlock")
                .arg("-c")
                .mode(Mode::Scope)
                .wrapped_argv(&Runner::Shell, None, &[]),
        );
        assert_eq!(argv, vec!["hyprlock", "-c"]);
    }

    #[test]
    fn a_singleton_keeps_its_name_and_an_instance_does_not() {
        let lock = Program::new("hyprlock")
            .tag("lock")
            .singleton(OnCollision::Refuse);
        assert_eq!(lock.unit_name(), lock.unit_name());
        assert_eq!(lock.unit_name(), "obayebar-lock.service");

        let app = Program::new("firefox").tag("app");
        assert_ne!(
            app.unit_name(),
            app.unit_name(),
            "two launches at once must not collide on one unit name"
        );
    }

    #[test]
    fn a_tag_is_folded_into_something_systemd_accepts() {
        assert_eq!(sanitize_tag("org.mozilla.firefox"), "org-mozilla-firefox");
        assert_eq!(sanitize_tag("with space/and slash"), "with-space-and-slash");
        assert_eq!(sanitize_tag("///"), "app");
        assert_eq!(sanitize_tag(""), "app");
        assert!(sanitize_tag(&"x".repeat(200)).len() <= 48);
    }

    #[test]
    fn a_configured_slice_replaces_the_default_in_the_argv() {
        let argv = strings(
            &Program::new("firefox")
                .slice("app-desktop.slice")
                .wrapped_argv(&systemd(), None, &[]),
        );
        assert!(argv.contains(&"--slice=app-desktop.slice".to_string()));
        assert!(!argv.contains(&format!("--slice={DEFAULT_SLICE}")));
    }

    #[test]
    fn a_slice_name_systemd_would_take() {
        assert!(valid_slice_name("app-obayebar.slice"));
        assert!(valid_slice_name("app.slice"));
        assert!(valid_slice_name("app-obayebar-launched.slice"));
        assert!(valid_slice_name("my_slice.slice"));
    }

    #[test]
    fn a_slice_name_systemd_would_refuse() {
        // Without the suffix systemd does not know it is a slice at all, and
        // the other shapes all break the dash-is-a-separator rule that makes
        // the name the position in the tree.
        assert!(!valid_slice_name("app-obayebar"));
        assert!(!valid_slice_name("app-obayebar.service"));
        assert!(!valid_slice_name(".slice"));
        assert!(!valid_slice_name("-leading.slice"));
        assert!(!valid_slice_name("trailing-.slice"));
        assert!(!valid_slice_name("double--dash.slice"));
        assert!(!valid_slice_name("with space.slice"));
        assert!(!valid_slice_name(""));
        assert!(!valid_slice_name(&format!("{}.slice", "x".repeat(255))));
    }

    #[test]
    fn a_rejected_name_leaves_the_default_in_place() {
        // A mistyped line in a config file would otherwise make systemd-run
        // refuse every request, and the launcher would launch nothing at all.
        install(&crate::config::SpawnConfig {
            slice: Some("not a slice".to_string()),
        });
        assert_eq!(configured_slice(), DEFAULT_SLICE);
    }

    #[test]
    fn a_config_that_names_no_slice_changes_nothing() {
        install(&crate::config::SpawnConfig::default());
        assert_eq!(configured_slice(), DEFAULT_SLICE);
    }

    #[test]
    fn a_missing_wrapper_is_reported_against_the_program_it_would_have_started() {
        // The caller asked for firefox, not for systemd-run; naming the
        // wrapper in the message would send them looking in the wrong place.
        let error = Program::new("firefox")
            .run(
                &Runner::Systemd(PathBuf::from("/nonexistent/systemd-run")),
                None,
            )
            .unwrap_err();
        assert!(
            matches!(&error, Error::Io { program, .. } if program == "firefox"),
            "got {error:?}"
        );
    }

    #[test]
    fn a_wrapper_that_says_no_carries_its_own_words() {
        // Standing in for a systemd that rejects a property it does not know:
        // sh gets systemd-run's arguments and refuses them. What matters is
        // that the refusal reaches the caller instead of being swallowed with
        // the piped stderr.
        let Some(sh) = find_in_path("sh") else {
            return;
        };
        let error = Program::new("firefox")
            .run(&Runner::Systemd(sh), None)
            .unwrap_err();
        let Error::Refused {
            wrapper,
            program,
            reason,
        } = error
        else {
            panic!("a non-zero wrapper should be a refusal");
        };
        assert_eq!(wrapper, "systemd-run");
        assert_eq!(program, "firefox");
        assert!(
            !reason.is_empty(),
            "the wrapper's complaint is the useful part"
        );
    }

    #[test]
    fn an_empty_program_is_refused_before_anything_runs() {
        assert!(matches!(
            Program::from_argv(Vec::<String>::new()).spawn(),
            Err(Error::Empty)
        ));
        assert!(matches!(Program::new("").command(), Err(Error::Empty)));
    }
}
