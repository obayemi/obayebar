//! Bar-surface reconciliation: keeping one layer-shell bar per connected
//! monitor, verified against what the compositor actually shows.
//!
//! [`BarFleet`] owns every field the reconcile loop touches and is the only
//! thing allowed to mutate them, a module boundary rather than a doc
//! comment's promise. [`plan`] holds the pure planner this fleet drives,
//! independently testable without a compositor.

mod plan;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::style;
use iced::window;
use iced_layershell::reexport::{
    Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings, OutputOption,
};
use obayebar_core::hypr::LayerMap;

use plan::{
    plan_from_observation, should_reissue_close, BarRecord, BarState, CloseReason, ClosingRecord,
    ForgetReason,
};

/// Prefix for every bar's layer-shell namespace, followed by this process's pid
/// and the bar's generation.
///
/// The pid scopes the namespaces to one instance. Bars are matched back from
/// `j/layers`, which shows every client's surfaces including those of a second
/// obayebar someone started by hand — and with a shared prefix and a generation
/// counter that restarts at zero, two instances would name surfaces
/// identically and each would happily verify its bars against the other's.
///
/// User-visible: a Hyprland `layerrule` that matched the exact namespace
/// `obayebar` must match a prefix instead, e.g. `layerrule = blur, ^obayebar`.
const BAR_NAMESPACE_PREFIX: &str = "obayebar-bar-";

/// Base delay before checking whether a bar landed where we asked. Long enough
/// for the compositor to map a surface we just requested.
const VERIFY_DELAY: Duration = Duration::from_millis(250);

/// Ceiling for the verification backoff, so a compositor that persistently
/// refuses to place a surface is retried slowly rather than in a hot loop.
const MAX_VERIFY_BACKOFF: Duration = Duration::from_secs(30);

/// What a closed window means for the fleet.
pub enum BarClosed {
    /// Not one of our bars, or one whose tracking is already cleared.
    /// Panels and popups close through here too, so it never calls for a
    /// respawn.
    NotABar,
    /// A close we asked for landed.
    ClosedAsRequested,
    /// The compositor closed a bar surface on its own; `uncovered` names its
    /// monitor if no other bar is left on it.
    Died { uncovered: Option<String> },
}

/// What a reconcile pass decided, translated into what the caller must do.
///
/// `App` turns this into `Task`s and drops the per-monitor workspace state
/// named in `drop_state_for` — the cross-domain state this module does not
/// own.
pub struct ReconcileOutcome {
    /// Surfaces to ask the compositor to close: new closes and reissued ones.
    pub close_ids: Vec<window::Id>,
    /// Monitors whose per-monitor state (workspace spring, canvas cache)
    /// should be dropped, because no bar covers them any more.
    pub drop_state_for: Vec<String>,
    /// A bar to spawn this pass, already recorded, settings ready to hand to
    /// `NewLayerShell`.
    pub spawn: Option<(window::Id, NewLayerShellSettings)>,
    /// Whether another verification pass is warranted.
    pub needs_verify: bool,
}

/// Every bar surface this instance has asked the compositor for, and the
/// state the reconcile loop needs to keep that in sync with reality.
///
/// Fields are private: only the methods here may touch them, which is what
/// keeps the reconcile invariant ("only one pass in flight") from being
/// violated by code elsewhere in the app.
#[derive(Debug)]
pub struct BarFleet {
    /// Every bar surface we have asked the compositor for, keyed by window id.
    ///
    /// This is bookkeeping, not truth: a record says which monitor we *asked*
    /// for, and `BarRecord::state` says whether the compositor was ever
    /// observed agreeing.
    tracked: HashMap<window::Id, BarRecord>,
    /// Bar surfaces we have asked the compositor to close, kept until an
    /// observation shows they are actually gone.
    ///
    /// A close is verified like any other request: a surface dropped from
    /// tracking the moment we *ask* for its close can still map a moment
    /// later, and once that happens nothing tracks it and nothing can reach
    /// it to close it again.
    closing: HashMap<window::Id, ClosingRecord>,
    /// Namespace prefix for this instance's bars: [`BAR_NAMESPACE_PREFIX`] and
    /// our pid. Also what tells our surfaces from another instance's.
    prefix: String,
    /// Monotonic counter making each bar's layer-shell namespace unique, so a
    /// `j/layers` observation can be matched back to one specific surface.
    generation: u64,
    /// Whether a verification pass is already scheduled.
    ///
    /// Every monitor-set change asks for one, and a Hyprland hotplug emits a
    /// burst of those; this keeps at most one `j/layers` query in flight
    /// instead of several reconciling the same state at once.
    verify_pending: bool,
    /// Delay before the next verification pass. Grows while the compositor
    /// has not honoured a request — a spawn that never appeared, or a close
    /// that has not landed — so a refusing compositor is not hammered; reset
    /// whenever the monitor set changes or a bar verifies.
    verify_backoff: Duration,
}

impl BarFleet {
    pub fn new() -> Self {
        Self {
            tracked: HashMap::new(),
            closing: HashMap::new(),
            prefix: format!("{BAR_NAMESPACE_PREFIX}{}-", std::process::id()),
            generation: 0,
            verify_pending: false,
            verify_backoff: VERIFY_DELAY,
        }
    }

    /// Get the monitor name for a bar window id. `None` if `id` is not a
    /// tracked bar surface.
    pub fn monitor_for(&self, id: window::Id) -> Option<&str> {
        self.tracked.get(&id).map(|record| record.monitor.as_str())
    }

    /// Schedule a verification pass unless one is already in flight. Returns
    /// the delay to wait before running it, or `None` when a pass is already
    /// pending.
    pub const fn begin_verify(&mut self) -> Option<Duration> {
        if self.verify_pending {
            return None;
        }
        self.verify_pending = true;
        Some(self.verify_backoff)
    }

    /// Return to the fastest verification rate.
    pub const fn reset_backoff(&mut self) {
        self.verify_backoff = VERIFY_DELAY;
    }

    /// Tell the fleet a window closed, and get back what it means.
    pub fn on_closed(&mut self, id: window::Id) -> BarClosed {
        if let Some(record) = self.closing.remove(&id) {
            log::info!("bars: {} closed as requested", record.namespace);
            return BarClosed::ClosedAsRequested;
        }
        let Some(record) = self.tracked.remove(&id) else {
            return BarClosed::NotABar;
        };
        log::info!(
            "bars: {} on {} was closed by the compositor",
            record.namespace,
            record.monitor
        );
        let uncovered = (!self.has_bar_on(&record.monitor)).then_some(record.monitor);
        BarClosed::Died { uncovered }
    }

    /// Whether any tracked bar is on `monitor`.
    fn has_bar_on(&self, monitor: &str) -> bool {
        self.tracked.values().any(|r| r.monitor == monitor)
    }

    /// Record a new bar aimed at `monitor` under a unique namespace, and
    /// build the layer-shell settings the caller spawns it with.
    ///
    /// The namespace is the whole point: `OutputOption::OutputName` is a
    /// request, not a guarantee — on a name-cache miss layershellev creates the
    /// surface with no output and the compositor puts it on the focused
    /// monitor, reporting nothing back. A per-surface namespace is what lets
    /// the next `j/layers` observation say which monitor this specific surface
    /// landed on — a namespace shared across bars would make them
    /// indistinguishable, and verification impossible.
    fn spawn_for(&mut self, monitor: String, now: Instant) -> (window::Id, NewLayerShellSettings) {
        self.generation = self.generation.wrapping_add(1);
        let namespace = format!("{}{}", self.prefix, self.generation);
        let id = window::Id::unique();
        log::info!(
            "bars: spawning {namespace} for {monitor} (generation {})",
            self.generation
        );
        self.tracked.insert(
            id,
            BarRecord {
                monitor: monitor.clone(),
                namespace: namespace.clone(),
                state: BarState::Mapping { spawned_at: now },
            },
        );
        let settings = NewLayerShellSettings {
            anchor: Anchor::Left | Anchor::Top | Anchor::Bottom,
            layer: Layer::Top,
            exclusive_zone: Some(style::BAR_EXCLUSIVE_ZONE),
            size: Some((style::BAR_WIDTH, 0)),
            output_option: OutputOption::OutputName(monitor),
            keyboard_interactivity: KeyboardInteractivity::None,
            namespace: Some(namespace),
            ..NewLayerShellSettings::default()
        };
        (id, settings)
    }

    /// Reconcile bars against what the compositor says is on screen.
    ///
    /// `observed` is the `j/layers` answer, or `None` if the query failed.
    /// Enforces three invariants over the connected monitors:
    ///
    /// 1. Every connected monitor has a bar on it.
    /// 2. No bar is left over for a disconnected monitor.
    /// 3. No two bars share a monitor.
    ///
    /// Everything here is driven by `observed`, never by this fleet's own
    /// tracking: checking tracking against itself cannot detect a broken
    /// state.
    pub fn reconcile(
        &mut self,
        observed: Option<&LayerMap>,
        expected: &HashSet<String>,
        now: Instant,
    ) -> ReconcileOutcome {
        self.verify_pending = false;
        let plan = plan_from_observation(
            observed,
            expected,
            &self.tracked,
            &self.closing,
            &self.prefix,
            now,
        );

        self.apply_verified(&plan.verified);
        log_pending(&self.tracked, &plan.pending);
        let mut close_ids = self.apply_closes(&plan.close);
        self.apply_forgets(&plan.forget);
        close_ids.extend(self.apply_closing(&plan.closing_observed, &plan.closing_gone));
        log_orphans(&plan.orphans);

        // One spawn per pass, on purpose. Batching several put them all into
        // one `Task::batch`, which layershellev drains inside
        // `process_window_state` — a context that cannot dispatch the wayland
        // queue at all, so every spawn resolved its output name against the
        // same frozen cache. When that cache was cold they all missed
        // together and stacked on the focused monitor. Spawning one at a time
        // and verifying in between makes that impossible.
        let spawn = plan.spawn.map(|monitor| self.spawn_for(monitor, now));

        self.log_invariants(observed, expected);

        // Keep verifying while anything is unconfirmed or uncovered. Once
        // every monitor has a verified bar this stops scheduling, so a
        // settled multi-monitor setup costs nothing.
        let needs_verify = self.needs_verification(expected);

        ReconcileOutcome {
            close_ids,
            drop_state_for: plan.drop_state_for,
            spawn,
            needs_verify,
        }
    }

    /// Mark newly-observed records verified, resetting the backoff on the
    /// transition into that state.
    fn apply_verified(&mut self, verified: &[window::Id]) {
        for id in verified {
            if let Some(record) = self.tracked.get_mut(id) {
                if matches!(record.state, BarState::Verified) {
                    continue;
                }
                log::info!("bars: {} confirmed on {}", record.namespace, record.monitor);
                record.state = BarState::Verified;
                // Reset only on the transition: re-confirming an
                // already-verified bar is not progress, and treating it as
                // such would hold the backoff at its minimum while a stuck
                // surface is polled at the fastest rate forever.
                self.reset_backoff();
            }
        }
    }

    /// Move each closed record into the closing set and collect the ids to
    /// ask the compositor to close.
    fn apply_closes(&mut self, closes: &[(window::Id, CloseReason)]) -> Vec<window::Id> {
        let mut close_ids = Vec::with_capacity(closes.len());
        for (id, reason) in closes {
            if let Some(record) = self.tracked.remove(id) {
                log::warn!(
                    "bars: closing {} ({reason}); wanted {}",
                    record.namespace,
                    record.monitor
                );
                if *reason == CloseReason::NeverAppeared {
                    // The spawn did not take. Ask less often before trying the
                    // next one, so a compositor that will not place our
                    // surfaces is not driven in a tight spawn/close loop.
                    self.grow_backoff();
                }
                self.closing.insert(
                    *id,
                    ClosingRecord {
                        namespace: record.namespace,
                        attempts: 0,
                    },
                );
            }
            close_ids.push(*id);
        }
        close_ids
    }

    /// Drop each record whose surface the observation shows is genuinely
    /// gone, deliberately without a close request.
    fn apply_forgets(&mut self, forgets: &[(window::Id, ForgetReason)]) {
        for (id, reason) in forgets {
            if let Some(record) = self.tracked.remove(id) {
                log::warn!("bars: forgetting {} ({reason})", record.namespace);
            }
        }
    }

    /// Re-request a close still observed mapped, drop one no longer seen, and
    /// collect the ids to re-ask. Every surface still mapped grows the
    /// backoff, whether or not it is re-asked this pass — a surface that will
    /// not go away must not hold the poll at its fastest rate for the rest of
    /// the session. One that is gone does not: it cost nothing to wait for.
    fn apply_closing(
        &mut self,
        closing_observed: &[window::Id],
        closing_gone: &[window::Id],
    ) -> Vec<window::Id> {
        let mut close_ids = Vec::new();
        for id in closing_observed {
            if let Some(record) = self.closing.get_mut(id) {
                record.attempts = record.attempts.saturating_add(1);
                if should_reissue_close(record.attempts) {
                    log::warn!(
                        "bars: {} still mapped after {} passes; asking again",
                        record.namespace,
                        record.attempts
                    );
                    close_ids.push(*id);
                }
            }
            self.grow_backoff();
        }
        for id in closing_gone {
            if let Some(record) = self.closing.remove(id) {
                log::info!("bars: {} is gone", record.namespace);
            }
        }
        close_ids
    }

    /// Whether another verification pass is warranted.
    ///
    /// A pending close counts: until an observation says the surface is gone,
    /// the close is a request nobody has confirmed, and stopping there is what
    /// left surfaces on screen with no one watching for them.
    fn needs_verification(&self, expected: &HashSet<String>) -> bool {
        !self.closing.is_empty()
            || self
                .tracked
                .values()
                .any(|r| !matches!(r.state, BarState::Verified))
            || expected.iter().any(|m| !self.has_verified_bar_on(m))
    }

    /// Whether a verified bar is on `monitor`.
    fn has_verified_bar_on(&self, monitor: &str) -> bool {
        self.tracked
            .values()
            .any(|r| r.monitor == monitor && r.state == BarState::Verified)
    }

    /// Slow the next pass down after a request the compositor has not
    /// honoured: a spawn that never appeared, or a close that has not landed.
    fn grow_backoff(&mut self) {
        self.verify_backoff = self
            .verify_backoff
            .saturating_mul(2)
            .min(MAX_VERIFY_BACKOFF);
    }

    /// Report invariant violations against the *observation*, always — not
    /// against our own tracking. A self-referential check would report
    /// success in every broken state.
    fn log_invariants(&self, observed: Option<&LayerMap>, expected: &HashSet<String>) {
        let Some(observed) = observed else {
            return;
        };
        let ours: HashSet<&str> = self
            .tracked
            .values()
            .map(|r| r.namespace.as_str())
            .collect();
        let mut per_monitor: HashMap<&str, usize> = HashMap::new();
        for (monitor, namespaces) in observed {
            let count = namespaces
                .iter()
                .filter(|ns| ours.contains(ns.as_str()))
                .count();
            if count > 0 {
                per_monitor.insert(monitor.as_str(), count);
            }
        }
        for (monitor, count) in &per_monitor {
            if *count > 1 {
                log::error!("bar invariant: {count} bars observed on monitor {monitor}");
            }
            if !expected.contains(*monitor) {
                log::error!("bar invariant: bar observed on unexpected monitor {monitor}");
            }
        }
        for monitor in expected {
            if !per_monitor.contains_key(monitor.as_str()) {
                // Normal while a spawn is still in flight; only a settled
                // state with no bar is a real violation, which the repeated
                // verification passes will surface as a persistent message.
                log::debug!("bars: monitor {monitor} has no bar yet");
            }
        }
    }
}

/// Log each record still inside its grace window, waiting to be observed.
fn log_pending(tracked: &HashMap<window::Id, BarRecord>, pending: &[window::Id]) {
    for id in pending {
        if let Some(record) = tracked.get(id) {
            log::debug!(
                "bars: still waiting for {} on {}",
                record.namespace,
                record.monitor
            );
        }
    }
}

/// Orphaned bar surfaces are reported, not actioned: nothing in this process
/// can close one.
fn log_orphans(orphans: &[String]) {
    for namespace in orphans {
        log::error!("bar invariant: untracked bar surface {namespace} on screen");
    }
}

/// Builders shared by this module's and [`plan`]'s tests, so a monitor set or
/// a `j/layers` observation is shaped the same way on both sides of the
/// planner boundary.
#[cfg(test)]
pub mod test_support {
    use std::collections::HashSet;

    pub fn expected<const N: usize>(monitors: [&str; N]) -> HashSet<String> {
        monitors.iter().map(|m| (*m).to_string()).collect()
    }

    pub fn observed<const N: usize>(
        entries: [(&str, &[&str]); N],
    ) -> obayebar_core::hypr::LayerMap {
        entries
            .into_iter()
            .map(|(monitor, namespaces)| {
                (
                    monitor.to_string(),
                    namespaces.iter().map(|n| (*n).to_string()).collect(),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod fleet_tests {
    use super::test_support::{expected, observed};
    use super::{BarClosed, BarFleet, BarRecord, BarState, ClosingRecord, VERIFY_DELAY};
    use iced::window;
    use iced_layershell::reexport::OutputOption;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    /// A fleet with one tracked record, backed off well past the minimum so
    /// a reset is observable.
    fn fleet_with(id: window::Id, monitor: &str, namespace: &str, state: BarState) -> BarFleet {
        BarFleet {
            tracked: HashMap::from([(
                id,
                BarRecord {
                    monitor: monitor.to_string(),
                    namespace: namespace.to_string(),
                    state,
                },
            )]),
            closing: HashMap::new(),
            prefix: "obayebar-bar-".to_string(),
            generation: 0,
            verify_pending: false,
            verify_backoff: Duration::from_secs(8),
        }
    }

    /// A fleet with one unrelated verified bar on DP-2, and `id` already
    /// closing under the `obayebar-bar-1` namespace with `attempts` passes
    /// spent still seeing it.
    fn fleet_closing(id: window::Id, attempts: u32) -> BarFleet {
        let mut fleet = fleet_with(
            window::Id::unique(),
            "DP-2",
            "unrelated",
            BarState::Verified,
        );
        fleet.closing.insert(
            id,
            ClosingRecord {
                namespace: "obayebar-bar-1".to_string(),
                attempts,
            },
        );
        fleet
    }

    #[test]
    fn verifying_a_pending_bar_resets_backoff() {
        let id = window::Id::unique();
        let mut fleet = fleet_with(
            id,
            "DP-1",
            "obayebar-bar-1",
            BarState::Mapping {
                spawned_at: Instant::now(),
            },
        );
        let obs = observed([("DP-1", &["obayebar-bar-1"])]);
        fleet.reconcile(Some(&obs), &expected(["DP-1"]), Instant::now());

        assert!(matches!(
            fleet.tracked.get(&id).map(|r| &r.state),
            Some(BarState::Verified)
        ));
        assert_eq!(fleet.verify_backoff, VERIFY_DELAY);
    }

    #[test]
    fn reverifying_an_already_verified_bar_keeps_backoff() {
        let id = window::Id::unique();
        let mut fleet = fleet_with(id, "DP-1", "obayebar-bar-1", BarState::Verified);
        let grown = fleet.verify_backoff;
        let obs = observed([("DP-1", &["obayebar-bar-1"])]);
        fleet.reconcile(Some(&obs), &expected(["DP-1"]), Instant::now());

        assert_eq!(fleet.verify_backoff, grown);
    }

    #[test]
    fn a_bar_that_never_appeared_is_closed_and_grows_backoff() -> Result<(), &'static str> {
        let id = window::Id::unique();
        let spawned_at = Instant::now()
            .checked_sub(Duration::from_secs(10))
            .ok_or("instant far enough in the past")?;
        let mut fleet = fleet_with(
            id,
            "DP-1",
            "obayebar-bar-1",
            BarState::Mapping { spawned_at },
        );
        let before = fleet.verify_backoff;
        let obs = observed([]);
        let outcome = fleet.reconcile(Some(&obs), &expected(["DP-1"]), Instant::now());

        assert_eq!(outcome.close_ids, vec![id]);
        assert!(!fleet.tracked.contains_key(&id));
        assert!(fleet.closing.contains_key(&id));
        assert!(fleet.verify_backoff > before);
        Ok(())
    }

    #[test]
    fn a_disconnected_bar_is_closed_without_growing_backoff() {
        let id = window::Id::unique();
        let mut fleet = fleet_with(id, "DP-1", "obayebar-bar-1", BarState::Verified);
        let before = fleet.verify_backoff;
        let obs = observed([("DP-1", &["obayebar-bar-1"])]);
        let outcome = fleet.reconcile(Some(&obs), &expected(["DP-2"]), Instant::now());

        assert_eq!(outcome.close_ids, vec![id]);
        assert!(fleet.closing.contains_key(&id));
        assert_eq!(fleet.verify_backoff, before);
    }

    #[test]
    fn a_vanished_bar_is_forgotten_without_a_close_request() {
        let id = window::Id::unique();
        let mut fleet = fleet_with(id, "DP-1", "obayebar-bar-1", BarState::Verified);
        let outcome = fleet.reconcile(Some(&observed([])), &expected(["DP-1"]), Instant::now());

        assert_eq!(outcome.close_ids, Vec::new());
        assert!(!fleet.tracked.contains_key(&id));
        assert!(!fleet.closing.contains_key(&id));
    }

    #[test]
    fn a_closing_surface_still_observed_is_reissued_and_grows_backoff() {
        let id = window::Id::unique();
        let mut fleet = fleet_closing(id, 0);
        let before = fleet.verify_backoff;
        let obs = observed([("DP-1", &["obayebar-bar-1"])]);
        let outcome = fleet.reconcile(Some(&obs), &expected(["DP-1", "DP-2"]), Instant::now());

        assert_eq!(outcome.close_ids, vec![id]);
        assert_eq!(fleet.closing.get(&id).map(|r| r.attempts), Some(1));
        assert!(fleet.verify_backoff > before);
    }

    #[test]
    fn a_closing_surface_not_due_for_reissue_still_grows_backoff() {
        let id = window::Id::unique();
        let mut fleet = fleet_closing(id, 2);
        let before = fleet.verify_backoff;
        let obs = observed([("DP-1", &["obayebar-bar-1"])]);
        let outcome = fleet.reconcile(Some(&obs), &expected(["DP-1", "DP-2"]), Instant::now());

        assert_eq!(outcome.close_ids, Vec::new());
        assert_eq!(fleet.closing.get(&id).map(|r| r.attempts), Some(3));
        assert!(fleet.verify_backoff > before);
    }

    #[test]
    fn a_gone_closing_surface_is_dropped() {
        let id = window::Id::unique();
        let mut fleet = fleet_closing(id, 1);
        let before = fleet.verify_backoff;
        let obs = observed([("DP-2", &["unrelated"])]);
        let outcome = fleet.reconcile(Some(&obs), &expected(["DP-2"]), Instant::now());

        assert_eq!(outcome.close_ids, Vec::new());
        assert!(!fleet.closing.contains_key(&id));
        assert_eq!(fleet.verify_backoff, before);
    }

    #[test]
    fn an_uncovered_monitor_is_spawned_for() -> Result<(), &'static str> {
        let mut fleet = BarFleet::new();
        let outcome = fleet.reconcile(Some(&observed([])), &expected(["DP-1"]), Instant::now());

        let (id, settings) = outcome.spawn.ok_or("a spawn was planned")?;
        assert_eq!(
            settings.output_option,
            OutputOption::OutputName("DP-1".to_string())
        );
        assert_eq!(settings.exclusive_zone, Some(54));
        assert!(fleet.tracked.contains_key(&id));
        assert!(outcome.needs_verify);
        Ok(())
    }

    #[test]
    fn begin_verify_allows_only_one_pass_in_flight() {
        let mut fleet = BarFleet::new();
        assert_eq!(fleet.begin_verify(), Some(VERIFY_DELAY));
        assert_eq!(fleet.begin_verify(), None);

        fleet.reconcile(Some(&observed([])), &expected(["DP-1"]), Instant::now());

        assert_eq!(fleet.begin_verify(), Some(VERIFY_DELAY));
    }

    #[test]
    fn on_closed_is_not_a_bar_for_an_untracked_id() {
        let mut fleet = BarFleet::new();
        assert!(matches!(
            fleet.on_closed(window::Id::unique()),
            BarClosed::NotABar
        ));
    }

    #[test]
    fn on_closed_reports_a_landed_close_and_forgets_it() {
        let id = window::Id::unique();
        let mut fleet = fleet_closing(id, 2);
        assert!(matches!(fleet.on_closed(id), BarClosed::ClosedAsRequested));
        assert!(!fleet.closing.contains_key(&id));
    }

    #[test]
    fn on_closed_reports_the_monitor_uncovered_when_no_bar_is_left() -> Result<(), &'static str> {
        let id = window::Id::unique();
        let mut fleet = fleet_with(id, "DP-1", "obayebar-bar-1", BarState::Verified);
        let BarClosed::Died { uncovered } = fleet.on_closed(id) else {
            return Err("expected Died");
        };
        assert_eq!(uncovered, Some("DP-1".to_string()));
        assert!(!fleet.tracked.contains_key(&id));
        Ok(())
    }

    #[test]
    fn on_closed_reports_no_uncovered_monitor_while_another_bar_covers_it(
    ) -> Result<(), &'static str> {
        let id = window::Id::unique();
        let mut fleet = fleet_with(id, "DP-1", "obayebar-bar-1", BarState::Verified);
        fleet.tracked.insert(
            window::Id::unique(),
            BarRecord {
                monitor: "DP-1".to_string(),
                namespace: "obayebar-bar-2".to_string(),
                state: BarState::Verified,
            },
        );
        let BarClosed::Died { uncovered } = fleet.on_closed(id) else {
            return Err("expected Died");
        };
        assert_eq!(uncovered, None);
        Ok(())
    }

    #[test]
    fn a_fully_verified_setup_needs_no_further_verification() {
        let id = window::Id::unique();
        let mut fleet = fleet_with(id, "DP-1", "obayebar-bar-1", BarState::Verified);
        let obs = observed([("DP-1", &["obayebar-bar-1"])]);
        let outcome = fleet.reconcile(Some(&obs), &expected(["DP-1"]), Instant::now());

        assert!(!outcome.needs_verify);
    }

    #[test]
    fn an_expected_monitor_with_no_bar_still_needs_verification() {
        let id = window::Id::unique();
        let mut fleet = fleet_with(id, "DP-1", "obayebar-bar-1", BarState::Verified);
        let outcome = fleet.reconcile(None, &expected(["DP-1", "DP-2"]), Instant::now());

        assert!(outcome.needs_verify);
    }
}
