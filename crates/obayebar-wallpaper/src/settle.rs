//! Chasing a monitor that has been announced but has no surface yet.
//!
//! Hyprland naming a new monitor and the compositor handing us the matching
//! `wl_output` are two separate arrivals with no ordering between them. A
//! selection pass that lands between the two has a monitor to decide for and
//! nothing to draw it on, so the assignment is dropped — and without a
//! follow-up that screen stays bare until the rotation timer next fires.
//! Watching both signals covers the common orderings; this covers the rest,
//! including a compositor that could not be asked at all.

use std::time::{Duration, Instant};

/// How long to wait before the first follow-up. Long enough for a compositor
/// that has just announced an output to finish creating it.
const FIRST_DELAY: Duration = Duration::from_millis(250);

/// Ceiling for the backoff, so a monitor that never materialises is looked at
/// occasionally rather than in a tight loop.
const MAX_DELAY: Duration = Duration::from_secs(4);

/// How long a monitor may stay uncovered before we stop chasing it.
///
/// Wall-clock, deliberately, and for the same reason the bar's verification
/// grace is: a dock hotplug fires a burst of events, so a budget counted in
/// passes is spent in a fraction of a second — long before a compositor busy
/// re-creating outputs has anything to show.
const GRACE: Duration = Duration::from_secs(10);

/// What a selection pass achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Every monitor the compositor lists has its picture.
    Complete,
    /// Something is still missing: a monitor with no surface, or a monitor
    /// list that could not be read.
    Waiting,
}

/// The follow-up schedule for passes that left something behind.
#[derive(Debug)]
pub struct Settle {
    /// When to stop chasing. Set on the first incomplete pass of a run.
    deadline: Option<Instant>,
    /// The delay the next follow-up gets.
    delay: Duration,
    /// A follow-up is already on its way, so a second one would only duplicate
    /// it. Overlapping chains are what a hotplug burst produces otherwise.
    armed: bool,
}

impl Default for Settle {
    fn default() -> Self {
        Self {
            deadline: None,
            delay: FIRST_DELAY,
            armed: false,
        }
    }
}

impl Settle {
    /// How long to wait before passing again, or `None` when there is nothing
    /// left to chase.
    pub fn schedule(&mut self, pass: Pass, now: Instant) -> Option<Duration> {
        if pass == Pass::Complete {
            self.rest();
            return None;
        }
        if self.armed {
            return None;
        }
        let deadline = *self
            .deadline
            .get_or_insert_with(|| now.checked_add(GRACE).unwrap_or(now));
        if now >= deadline {
            log::warn!(
                "wallpaper: a monitor is still without a surface after {}s, waiting for the next event",
                GRACE.as_secs()
            );
            self.rest();
            return None;
        }
        let delay = self.delay;
        self.delay = self
            .delay
            .checked_mul(2)
            .unwrap_or(MAX_DELAY)
            .min(MAX_DELAY);
        self.armed = true;
        Some(delay)
    }

    /// The follow-up we scheduled has started, so another may be queued behind
    /// it. Only this clears the flag: a pass that succeeds in the meantime
    /// must not let a second chain start while the first is still pending.
    pub const fn fired(&mut self) {
        self.armed = false;
    }

    /// Nothing to chase: forget the deadline and the backoff so the next
    /// hotplug starts from a clean schedule.
    const fn rest(&mut self) {
        self.deadline = None;
        self.delay = FIRST_DELAY;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{Pass, Settle, FIRST_DELAY, GRACE, MAX_DELAY};
    use std::time::{Duration, Instant};

    #[test]
    fn a_complete_pass_schedules_nothing() {
        let mut settle = Settle::default();
        assert_eq!(settle.schedule(Pass::Complete, Instant::now()), None);
    }

    #[test]
    fn an_incomplete_pass_backs_off_up_to_the_ceiling() {
        let mut settle = Settle::default();
        let now = Instant::now();
        let mut seen = Vec::new();
        for _ in 0..8 {
            let delay = settle.schedule(Pass::Waiting, now).unwrap();
            settle.fired();
            seen.push(delay);
        }
        assert_eq!(seen.first(), Some(&FIRST_DELAY));
        assert!(
            seen.windows(2).all(|pair| matches!(pair, [a, b] if b >= a)),
            "the delay never shrinks: {seen:?}"
        );
        assert_eq!(seen.last(), Some(&MAX_DELAY));
    }

    #[test]
    fn only_one_follow_up_is_ever_in_flight() {
        // A dock hotplug fires a burst of events, each of which passes. Every
        // one of them scheduling its own chain is how the retries multiply.
        let mut settle = Settle::default();
        let now = Instant::now();
        assert!(settle.schedule(Pass::Waiting, now).is_some());
        assert_eq!(settle.schedule(Pass::Waiting, now), None);
        settle.fired();
        assert!(settle.schedule(Pass::Waiting, now).is_some());
    }

    #[test]
    fn chasing_stops_after_the_grace_and_starts_fresh_next_time() {
        let mut settle = Settle::default();
        let start = Instant::now();
        assert!(settle.schedule(Pass::Waiting, start).is_some());
        settle.fired();

        let expired = start
            .checked_add(GRACE.saturating_add(Duration::from_secs(1)))
            .unwrap();
        assert_eq!(settle.schedule(Pass::Waiting, expired), None);

        // The next monitor change gets the whole grace again, from scratch.
        settle.fired();
        assert_eq!(settle.schedule(Pass::Waiting, expired), Some(FIRST_DELAY));
    }

    #[test]
    fn a_complete_pass_resets_the_backoff() {
        let mut settle = Settle::default();
        let now = Instant::now();
        assert!(settle.schedule(Pass::Waiting, now).is_some());
        settle.fired();
        assert!(settle.schedule(Pass::Waiting, now).unwrap() > FIRST_DELAY);
        settle.fired();

        assert_eq!(settle.schedule(Pass::Complete, now), None);
        assert_eq!(settle.schedule(Pass::Waiting, now), Some(FIRST_DELAY));
    }
}
