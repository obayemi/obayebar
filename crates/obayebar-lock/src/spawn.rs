//! Starting hyprlock, and surviving a bar restart while it runs.
//!
//! The isolation itself belongs to [`obayebar_core::spawn`], which every
//! obayebar-launched program goes through. What is specific here is *which*
//! knobs the lock screen needs and why:
//!
//! - a **scope**, not a service, because the point of running hyprlock is to
//!   find out when the screen was unlocked, and only a scope leaves the
//!   program as this process's child to wait on;
//! - a **singleton**, so a keybind pressed twice does not stack two lockers;
//! - **protected**, so systemd-oomd sheds the rest of the session before it
//!   sheds the thing standing between a stranger and the desktop.
//!
//! `--no-scope` opts out of all of it. It is a debugging flag, and the guards
//! below are what stop it from quietly producing a lock screen that
//! `systemctl --user restart` can kill.

use std::path::Path;
use std::process::Command;

use obayebar_core::spawn::{self, Mode, Program};

/// Env var naming the hyprlock binary, set by the Nix wrapper so the package
/// does not depend on the ambient PATH.
const HYPRLOCK_ENV: &str = "OBAYEBAR_HYPRLOCK";

/// Unit tag. Fixed rather than generated, so a second invocation is refused
/// instead of stacking two lock screens.
const TAG: &str = "lock";

/// What happened to hyprlock.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The screen was locked and has since been unlocked.
    Unlocked,
    /// Started, and we were asked not to wait.
    Detached,
    /// hyprlock ran but exited non-zero.
    Failed(Option<i32>),
    /// A lock is already up.
    AlreadyLocked,
    /// Could not start it at all.
    NotStarted(String),
}

/// How to launch it.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Wrap in a transient systemd scope. Off only for debugging.
    pub scope: bool,
    /// Return as soon as it starts rather than waiting for the unlock.
    pub detach: bool,
    /// Stop a lock screen that is already up instead of refusing to start.
    ///
    /// What the idle daemon asks for: a hyprlock still running is not proof
    /// that the screen is locked, and a hung one must not keep the session
    /// unlocked for as long as it lives.
    pub replace: bool,
    pub grace: Option<u32>,
}

/// The hyprlock binary to run.
fn binary() -> String {
    std::env::var(HYPRLOCK_ENV).unwrap_or_else(|_| "hyprlock".to_string())
}

/// Whether we are ourselves running inside a systemd unit.
///
/// If so, a caller opting out of the scope would put hyprlock in *that* unit's
/// cgroup, which is the exact trap the scope exists to avoid. Refusing is
/// better than silently producing a locker that a service restart can kill.
fn inside_a_unit() -> bool {
    std::env::var_os("INVOCATION_ID").is_some()
}

/// Why `--no-scope` cannot be honoured here, if it cannot.
///
/// Split out from [`lock`] so the rule is testable without a compositor: the
/// whole point is that it fires *before* anything is started.
fn refuse_unscoped(options: Options, inside_a_unit: bool) -> Option<String> {
    if options.scope || !inside_a_unit {
        return None;
    }
    Some(if options.detach {
        "--detach --no-scope inside a systemd unit would leave the lock killable".to_string()
    } else {
        "--no-scope inside a systemd unit would let a unit restart kill the lock screen".to_string()
    })
}

/// Run hyprlock against `config`.
pub fn lock(config: &Path, options: Options) -> Outcome {
    if let Some(why) = refuse_unscoped(options, inside_a_unit()) {
        return Outcome::NotStarted(why);
    }

    let hyprlock = binary();
    let mut command = if options.scope {
        let mut program = Program::new(&hyprlock)
            .tag(TAG)
            .mode(Mode::Scope)
            .singleton()
            .protected();
        if options.replace {
            program = program.replace();
        }
        match program.command() {
            Ok((command, _)) => command,
            Err(spawn::Error::AlreadyRunning { .. }) => return Outcome::AlreadyLocked,
            Err(e) => return Outcome::NotStarted(e.to_string()),
        }
    } else {
        Command::new(&hyprlock)
    };

    command.arg("-c").arg(config);
    if let Some(grace) = options.grace {
        command.arg("--grace").arg(grace.to_string());
    }

    if options.detach {
        return match command.spawn() {
            Ok(_) => Outcome::Detached,
            Err(e) => Outcome::NotStarted(format!("starting {hyprlock}: {e}")),
        };
    }

    match command.status() {
        Ok(status) if status.success() => Outcome::Unlocked,
        Ok(status) => Outcome::Failed(status.code()),
        Err(e) => Outcome::NotStarted(format!("starting {hyprlock}: {e}")),
    }
}

/// Turn an outcome into a process exit code and a message for the user.
///
/// Kept separate from [`lock`] so the mapping is testable without running
/// anything: these codes are what a keybind or an idle daemon reacts to.
#[must_use]
pub fn report(outcome: &Outcome) -> (i32, Option<String>) {
    match outcome {
        Outcome::Unlocked | Outcome::Detached => (0, None),
        Outcome::AlreadyLocked => (3, Some("a lock screen is already running".to_string())),
        Outcome::NotStarted(why) => (1, Some(why.clone())),
        // 134 is SIGABRT, which is how hyprlock exits when it cannot reach a
        // compositor — worth naming, because the bare number is baffling.
        Outcome::Failed(Some(134)) => (
            1,
            Some("hyprlock aborted; is there a Wayland compositor to connect to?".to_string()),
        ),
        Outcome::Failed(Some(code)) => (1, Some(format!("hyprlock exited {code}"))),
        Outcome::Failed(None) => (1, Some("hyprlock was killed by a signal".to_string())),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const fn options(scope: bool, detach: bool) -> Options {
        Options {
            scope,
            detach,
            replace: false,
            grace: None,
        }
    }

    #[test]
    fn success_and_detach_exit_zero_and_say_nothing() {
        assert_eq!(report(&Outcome::Unlocked), (0, None));
        assert_eq!(report(&Outcome::Detached), (0, None));
    }

    #[test]
    fn an_existing_lock_has_its_own_exit_code() {
        // Distinct from a failure: a keybind pressed twice is not an error, and
        // a caller may want to tell the two apart.
        let (code, message) = report(&Outcome::AlreadyLocked);
        assert_eq!(code, 3);
        assert!(message.is_some());
    }

    #[test]
    fn a_failure_to_start_is_reported_with_its_reason() {
        let (code, message) = report(&Outcome::NotStarted("no such binary".to_string()));
        assert_eq!(code, 1);
        assert_eq!(message.as_deref(), Some("no such binary"));
    }

    #[test]
    fn sigabrt_is_translated_rather_than_shown_raw() {
        let (code, message) = report(&Outcome::Failed(Some(134)));
        assert_eq!(code, 1);
        assert!(
            message.unwrap_or_default().contains("compositor"),
            "134 should be explained, not printed bare"
        );
    }

    #[test]
    fn other_exit_codes_are_passed_through_in_the_message() {
        let (_, message) = report(&Outcome::Failed(Some(2)));
        assert!(message.unwrap_or_default().contains('2'));
        let (_, signal) = report(&Outcome::Failed(None));
        assert!(signal.unwrap_or_default().contains("signal"));
    }

    #[test]
    fn the_binary_can_be_overridden_by_env() {
        // The Nix wrapper sets this so the package does not rely on PATH.
        assert_eq!(
            std::env::var(HYPRLOCK_ENV).unwrap_or_else(|_| "hyprlock".to_string()),
            binary()
        );
    }

    #[test]
    fn unscoped_inside_a_unit_is_refused_either_way() {
        // Both spellings produce a lock screen that `systemctl --user restart`
        // would kill, which unlocks the machine.
        assert!(refuse_unscoped(options(false, false), true).is_some());
        assert!(refuse_unscoped(options(false, true), true).is_some());
    }

    #[test]
    fn unscoped_outside_a_unit_is_allowed() {
        assert_eq!(refuse_unscoped(options(false, false), false), None);
        assert_eq!(refuse_unscoped(options(false, true), false), None);
    }

    #[test]
    fn a_scoped_lock_is_never_refused_for_its_cgroup() {
        assert_eq!(refuse_unscoped(options(true, false), true), None);
        assert_eq!(refuse_unscoped(options(true, true), true), None);
    }
}
