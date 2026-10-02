//! Bar-surface reconciliation: keeping one layer-shell bar per connected
//! monitor, verified against what the compositor actually shows.
//!
//! [`BarFleet`] owns every field the reconcile loop touches and is the only
//! thing allowed to mutate them — a module boundary enforces what used to be
//! only a doc comment's promise. [`plan`] holds the pure planner this fleet
//! drives, independently testable without a compositor.

mod plan;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::style;
use iced::window;
use iced_layershell::reexport::{
    Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings, OutputOption,
};
use obayebar_core::hypr::LayerMap;

use plan::{plan_from_observation, should_reissue_close, BarRecord, CloseReason, ClosingRecord};

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
/// keeps the reconcile invariants ("only one pass in flight", "a record's
/// `spawned_at` is meaningless once verified") from being violated by code
/// elsewhere in the app.
#[derive(Debug)]
pub struct BarFleet {
    /// Every bar surface we have asked the compositor for, keyed by window id.
    ///
    /// This is bookkeeping, not truth: a record says which monitor we *asked*
    /// for, and `BarRecord::verified` says whether the compositor was ever
    /// observed agreeing.
    tracked: HashMap<window::Id, BarRecord>,
    /// Bar surfaces we have asked the compositor to close, kept until an
    /// observation shows they are actually gone.
    ///
    /// Dropping a record at the moment we *ask* for the close is what let bars
    /// pile up: a surface that had not mapped yet was simply forgotten, and
    /// when it mapped a moment later nothing tracked it, nothing could close it
    /// and — since the tracking map looked consistent — nothing even looked
    /// again. A close is a request like any other, so it is verified like one.
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
    /// burst of those, so without this the passes ran concurrently: several
    /// `j/layers` queries in flight at once, all reconciling the same state.
    verify_pending: bool,
    /// Delay before the next verification pass. Grows when a spawn fails to
    /// appear so a compositor that refuses us is not hammered; reset whenever
    /// the monitor set changes or a bar verifies.
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

    /// Whether another verification pass is already scheduled. If not, this
    /// schedules one and returns the delay to wait before it.
    pub const fn begin_verify(&mut self) -> Option<Duration> {
        if self.verify_pending {
            return None;
        }
        self.verify_pending = true;
        Some(self.verify_backoff)
    }

    /// A fresh monitor topology is the one moment worth retrying eagerly, so
    /// drop any accumulated backoff.
    pub const fn reset_backoff(&mut self) {
        self.verify_backoff = VERIFY_DELAY;
    }

    /// Record that a close we asked for landed. `None` if `id` was not a bar
    /// we were closing.
    pub fn closing_remove(&mut self, id: window::Id) -> Option<ClosingRecord> {
        self.closing.remove(&id)
    }

    /// Record that the compositor closed a bar surface on its own. `None` if
    /// `id` was not one of our bars.
    pub fn remove_bar(&mut self, id: window::Id) -> Option<BarRecord> {
        self.tracked.remove(&id)
    }

    /// Whether any tracked bar is on `monitor`.
    pub fn has_bar_on(&self, monitor: &str) -> bool {
        self.tracked.values().any(|r| r.monitor == monitor)
    }

    /// Spawn one layer-shell bar aimed at `monitor`, under a unique namespace.
    ///
    /// The namespace is the whole point: `OutputOption::OutputName` is a
    /// request, not a guarantee — on a name-cache miss layershellev creates the
    /// surface with no output and the compositor puts it on the focused
    /// monitor, reporting nothing back. A per-surface namespace is what lets
    /// the next `j/layers` observation say which monitor this specific surface
    /// landed on. All bars previously shared the app-wide `obayebar`
    /// namespace, which made them indistinguishable and verification
    /// impossible.
    fn spawn_for(&mut self, monitor: String) -> (window::Id, NewLayerShellSettings) {
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
                verified: false,
                spawned_at: Instant::now(),
            },
        );
        let settings = NewLayerShellSettings {
            anchor: Anchor::Left | Anchor::Top | Anchor::Bottom,
            layer: Layer::Top,
            exclusive_zone: Some(i32::try_from(style::BAR_WIDTH).unwrap_or(54)),
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
    /// tracking — checking tracking against itself is exactly the mistake
    /// that reported success in every broken state.
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

        let mut close_ids = Vec::new();

        for id in &plan.verified {
            if let Some(record) = self.tracked.get_mut(id) {
                if !record.verified {
                    log::info!("bars: {} confirmed on {}", record.namespace, record.monitor);
                    // Something is working; stop backing off. Only on the
                    // transition: re-confirming a bar that was already fine is
                    // not progress, and treating it as such kept the backoff
                    // pinned at its minimum while a stuck surface was polled
                    // four times a second forever.
                    self.verify_backoff = VERIFY_DELAY;
                }
                record.verified = true;
            }
        }
        for id in &plan.pending {
            if let Some(record) = self.tracked.get(id) {
                log::debug!(
                    "bars: still waiting for {} on {}",
                    record.namespace,
                    record.monitor
                );
            }
        }
        for (id, reason) in &plan.close {
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
        for (id, reason) in &plan.forget {
            if let Some(record) = self.tracked.remove(id) {
                log::warn!("bars: forgetting {} ({reason})", record.namespace);
            }
            // Deliberately no close request: these are the records whose
            // surface the observation shows is genuinely gone.
        }
        for id in &plan.closing_observed {
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
            // A surface that will not go away must not hold the poll at its
            // fastest rate for the rest of the session.
            self.grow_backoff();
        }
        for id in &plan.closing_gone {
            if let Some(record) = self.closing.remove(id) {
                log::info!("bars: {} is gone", record.namespace);
            }
        }
        for namespace in &plan.orphans {
            log::error!("bar invariant: untracked bar surface {namespace} on screen");
        }

        // One spawn per pass, on purpose. Batching several put them all into
        // one `Task::batch`, which layershellev drains inside
        // `process_window_state` — a context that cannot dispatch the wayland
        // queue at all, so every spawn resolved its output name against the
        // same frozen cache. When that cache was cold they all missed
        // together and stacked on the focused monitor. Spawning one at a time
        // and verifying in between makes that impossible.
        let spawn = plan.spawn.map(|monitor| self.spawn_for(monitor));

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

    /// Whether another verification pass is warranted.
    ///
    /// A pending close counts: until an observation says the surface is gone,
    /// the close is a request nobody has confirmed, and stopping there is what
    /// left surfaces on screen with no one watching for them.
    fn needs_verification(&self, expected: &HashSet<String>) -> bool {
        let covered: HashSet<&str> = self
            .tracked
            .values()
            .filter(|r| r.verified)
            .map(|r| r.monitor.as_str())
            .collect();
        !self.closing.is_empty()
            || self.tracked.values().any(|r| !r.verified)
            || expected.iter().any(|m| !covered.contains(m.as_str()))
    }

    /// Back off after a spawn failed to appear, so a compositor that will not
    /// place our surface is retried at a decreasing rate rather than hammered.
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
