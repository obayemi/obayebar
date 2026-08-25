//! Starting hyprlock, and surviving a bar restart while it runs.
//!
//! The subtle part is the cgroup. `systemctl --user show obayebar.service`
//! reports `KillMode=control-group`, so anything the bar spawns as a plain
//! child lives in the bar's cgroup and is killed when that unit restarts. For a
//! lock screen that is not an inconvenience — `systemctl --user restart
//! obayebar` would **unlock the machine**. Wrapping hyprlock in its own
//! transient scope puts it in a different cgroup, where a bar restart cannot
//! reach it.

use std::path::Path;
use std::process::Command;

/// Env var naming the hyprlock binary, set by the Nix wrapper so the package
/// does not depend on the ambient PATH.
const HYPRLOCK_ENV: &str = "OBAYEBAR_HYPRLOCK";

/// Scope unit name. Fixed rather than generated, so a second invocation fails
/// instead of stacking two lock screens.
const SCOPE_UNIT: &str = "obayebar-lock";

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

/// Whether a lock scope is already running.
fn scope_is_active() -> bool {
    Command::new("systemctl")
        .args([
            "--user",
            "is-active",
            "--quiet",
            &format!("{SCOPE_UNIT}.scope"),
        ])
        .status()
        .is_ok_and(|s| s.success())
}

/// Clear a failed scope left by a previous run, which would otherwise make the
/// next `systemd-run --unit=` fail with "unit is already loaded".
fn reset_failed_scope() {
    let _ = Command::new("systemctl")
        .args(["--user", "reset-failed", &format!("{SCOPE_UNIT}.scope")])
        .status();
}

/// Run hyprlock against `config`.
pub fn lock(config: &Path, options: Options) -> Outcome {
    if options.scope && scope_is_active() {
        return Outcome::AlreadyLocked;
    }
    if !options.scope && inside_a_unit() {
        return Outcome::NotStarted(
            "--no-scope inside a systemd unit would let a unit restart kill the lock screen"
                .to_string(),
        );
    }
    if options.detach && inside_a_unit() && !options.scope {
        return Outcome::NotStarted(
            "--detach without a scope inside a systemd unit would leave the lock killable"
                .to_string(),
        );
    }

    let hyprlock = binary();
    let mut command = if options.scope {
        reset_failed_scope();
        let mut c = Command::new("systemd-run");
        c.args([
            "--user",
            "--scope",
            &format!("--unit={SCOPE_UNIT}"),
            // Tear the scope down as soon as it exits, so the next lock does
            // not trip over a lingering unit.
            "--collect",
            "--quiet",
            "--",
            &hyprlock,
        ]);
        c
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
}
