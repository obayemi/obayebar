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
//!
//! A takeover needs one more guard: the unit says a lock screen is up, never
//! whether it is the live one holding the session lock or one that hung
//! after an unlock. [`crate::lock_state`] asks the compositor instead,
//! and a takeover that would kill a live locker turns into
//! [`Outcome::AlreadyLocked`] before anything is touched.

use std::path::Path;
use std::process::Command;

use obayebar_core::spawn::{self, Mode, OnCollision, Program};

use crate::lock_state;

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
    /// Take over from a lock screen that is already up, rather than refusing
    /// to start. See [`OnCollision::Replace`] for why that is ever wanted.
    /// Never takes over one the compositor says is the live lock: see
    /// [`lock_state::session_locked`].
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
    if options.scope {
        return None;
    }
    if options.replace {
        // The singleton unit is the only thing a takeover can take over.
        return Some("--replace has no lock screen to replace without a scope".to_string());
    }
    if !inside_a_unit {
        return None;
    }
    Some(if options.detach {
        "--detach --no-scope inside a systemd unit would leave the lock killable".to_string()
    } else {
        "--no-scope inside a systemd unit would let a unit restart kill the lock screen".to_string()
    })
}

/// The isolated hyprlock to run, and what it does about one already running.
///
/// Split out of [`lock`] so that what the flag asks for is visible without a
/// systemd user manager to ask.
fn program(hyprlock: &str, replace: bool) -> Program {
    Program::new(hyprlock)
        .tag(TAG)
        .mode(Mode::Scope)
        .singleton(if replace {
            OnCollision::Replace
        } else {
            OnCollision::Refuse
        })
        .protected()
}

/// Run hyprlock against `config`.
pub fn lock(config: &Path, options: Options) -> Outcome {
    if let Some(why) = refuse_unscoped(options, inside_a_unit()) {
        return Outcome::NotStarted(why);
    }

    let hyprlock = binary();
    if !options.scope {
        return run(Command::new(&hyprlock), &hyprlock, config, options);
    }

    if options.replace && lock_state::session_locked() {
        return Outcome::AlreadyLocked;
    }

    let outcome = match program(&hyprlock, options.replace).command() {
        Ok((command, _)) => run(command, &hyprlock, config, options),
        Err(e) => claim_failed(e),
    };
    if !rescue_needed(&outcome, options.replace) {
        return outcome;
    }
    // A takeover can fail two ways: it stopped the old lock screen and the
    // replacement never started, or the old one refused to stop at all.
    // Either way the compositor may hold the session locked with no client,
    // which needs a VT to escape, so an unscoped hyprlock — one a unit
    // restart can kill — is the better of the two bad screens.
    log::warn!(
        "lock: the old lock screen would not stop or the replacement did not \
         start ({outcome:?}), retrying outside the scope"
    );
    run(Command::new(&hyprlock), &hyprlock, config, options)
}

/// Turn a failed claim into an [`Outcome`].
///
/// `AlreadyRunning` becomes [`Outcome::AlreadyLocked`], a shape [`run`]
/// itself never produces; every other error becomes [`Outcome::NotStarted`],
/// which is what lets a takeover that never claimed the unit reach
/// [`rescue_needed`] alongside one that claimed it and then failed to start.
fn claim_failed(error: spawn::Error) -> Outcome {
    match error {
        spawn::Error::AlreadyRunning { .. } => Outcome::AlreadyLocked,
        other => Outcome::NotStarted(other.to_string()),
    }
}

/// Whether a failed takeover has left the session locked behind no client.
///
/// Only a takeover: without one, nothing was stopped, and a second attempt
/// would just fail the same way.
const fn rescue_needed(outcome: &Outcome, replace: bool) -> bool {
    replace && matches!(outcome, Outcome::Failed(_) | Outcome::NotStarted(_))
}

/// Finish `command` into a hyprlock invocation and run it.
fn run(mut command: Command, hyprlock: &str, config: &Path, options: Options) -> Outcome {
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

    const fn replacing(scope: bool) -> Options {
        Options {
            scope,
            detach: false,
            replace: true,
            grace: None,
        }
    }

    #[test]
    fn the_flag_reaches_the_unit_that_has_to_give_way() {
        assert_eq!(
            program("hyprlock", true).on_collision(),
            Some(OnCollision::Replace)
        );
        assert_eq!(
            program("hyprlock", false).on_collision(),
            Some(OnCollision::Refuse)
        );
    }

    #[test]
    fn replacing_without_a_scope_is_refused_rather_than_ignored() {
        // There is no unit to take over, so honouring the flag would mean
        // starting a second hyprlock next to the one meant to give way.
        assert!(refuse_unscoped(replacing(false), false).is_some());
        assert!(refuse_unscoped(replacing(true), true).is_none());
    }

    #[test]
    fn only_a_takeover_that_failed_is_worth_a_second_attempt() {
        assert!(rescue_needed(&Outcome::Failed(Some(1)), true));
        assert!(rescue_needed(&Outcome::NotStarted(String::new()), true));
        assert!(!rescue_needed(&Outcome::Failed(Some(1)), false));
        assert!(!rescue_needed(&Outcome::Unlocked, true));
        assert!(!rescue_needed(&Outcome::AlreadyLocked, true));
    }

    #[test]
    fn a_unit_that_would_not_stop_is_worth_rescuing_too() {
        let outcome = claim_failed(spawn::Error::NotReplaced {
            unit: TAG.to_string(),
        });
        assert!(matches!(outcome, Outcome::NotStarted(_)));
        assert!(rescue_needed(&outcome, true));
    }

    #[test]
    fn a_unit_already_running_is_reported_as_already_locked_not_a_failure() {
        let outcome = claim_failed(spawn::Error::AlreadyRunning {
            unit: TAG.to_string(),
        });
        assert_eq!(outcome, Outcome::AlreadyLocked);
        assert!(!rescue_needed(&outcome, true));
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
