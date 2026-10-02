//! The pure bar-reconciliation planner.
//!
//! Everything here is a function of its inputs, so the whole state machine is
//! testable without a compositor — the bugs it needs to catch all lived in
//! the gap between a tracking map and reality.

use std::collections::HashMap;

use iced::window;

/// How long a freshly spawned bar gets to show up in `j/layers` before we give
/// up on it and replace it.
///
/// Wall-clock, deliberately, rather than a number of verification passes. A
/// pass count was the first shape and it was wrong: passes are scheduled by
/// anything that changes the monitor set, so during a dock hotplug several
/// chains overlap and burn the entire budget inside a fraction of a second —
/// long before a compositor busy re-creating outputs has mapped anything. Two
/// seconds comfortably covers a hotplug map without leaving a monitor bare.
pub const VERIFY_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// One bar surface we have asked the compositor for.
///
/// The distinction that matters: `monitor` is what we *requested*, `verified`
/// is whether the compositor was ever observed agreeing. Treating the request
/// as the truth is what produced bars stacked on one screen while the app
/// believed they were spread across all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarRecord {
    /// Monitor this bar was spawned for.
    pub monitor: String,
    /// Unique layer-shell namespace, our handle for matching `j/layers` output
    /// back to this specific surface.
    pub namespace: String,
    /// Set once this namespace was observed on `monitor`.
    pub verified: bool,
    /// When the spawn was requested. An unobserved surface is given the benefit
    /// of the doubt for `VERIFY_GRACE` from here, so one that never maps does
    /// not mask its monitor forever. Meaningless once `verified`.
    pub spawned_at: std::time::Instant,
}

/// A bar surface we have asked the compositor to close.
///
/// It keeps the window id — the only handle that can close the surface — until
/// an observation confirms the surface is gone. Anything dropped before that is
/// unreachable for the rest of the process's life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosingRecord {
    /// Namespace to look for in `j/layers`; absence is what ends the wait.
    pub namespace: String,
    /// Passes spent still seeing it, which paces the re-requests.
    pub attempts: u32,
}

/// Why `plan_from_observation` is closing a bar surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CloseReason {
    MonitorDisconnected,
    DuplicateOnMonitor,
    WrongMonitor,
    NeverAppeared,
}

impl std::fmt::Display for CloseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MonitorDisconnected => "monitor disconnected",
            Self::DuplicateOnMonitor => "duplicate on monitor",
            Self::WrongMonitor => "landed on the wrong monitor",
            Self::NeverAppeared => "never appeared",
        })
    }
}

/// Why `plan_from_observation` is dropping a bar record without closing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ForgetReason {
    MonitorDisconnected,
    SurfaceVanished,
}

impl std::fmt::Display for ForgetReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MonitorDisconnected => "monitor disconnected",
            Self::SurfaceVanished => "surface vanished",
        })
    }
}

/// What `plan_from_observation` decided. Every field is sorted so the plan is
/// deterministic and directly comparable in tests.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BarPlan {
    /// Surfaces to close, with the reason. The caller moves each into the
    /// closing set and keeps it there until an observation says it is gone.
    pub close: Vec<(window::Id, CloseReason)>,
    /// Records to drop without closing: the surface is already gone.
    pub forget: Vec<(window::Id, ForgetReason)>,
    /// Records observed where we asked for them.
    pub verified: Vec<window::Id>,
    /// Records not observed yet but still inside their grace window.
    pub pending: Vec<window::Id>,
    /// Closing surfaces the compositor still shows; the close has not taken
    /// effect yet, so the caller keeps waiting and re-asks now and then.
    pub closing_observed: Vec<window::Id>,
    /// Closing surfaces no longer anywhere in the observation. Done.
    pub closing_gone: Vec<window::Id>,
    /// Bar namespaces on screen that belong to neither set. Nothing in this
    /// process can close one, so it is reported, not actioned — it means a
    /// surface escaped tracking and the bug is upstream of here.
    pub orphans: Vec<String>,
    /// Monitors whose per-monitor state should be dropped.
    pub drop_state_for: Vec<String>,
    /// The single monitor to spawn a bar for this pass, if any.
    pub spawn: Option<String>,
}

/// Decide what to do about the bars, given what the compositor reports.
///
/// Pure, so the whole state machine is testable without a compositor: this is
/// what the old belief-only planner could not offer, because the bugs it needed
/// to catch all lived in the gap between the tracking map and reality.
///
/// `observed` maps monitor name to the layer namespaces mapped there. `None`
/// means the query failed. `now` is the clock the grace window is measured
/// against, passed in so this stays a pure function of its inputs.
pub fn plan_from_observation(
    observed: Option<&obayebar_core::hypr::LayerMap>,
    expected: &std::collections::HashSet<String>,
    tracked: &HashMap<window::Id, BarRecord>,
    closing: &HashMap<window::Id, ClosingRecord>,
    prefix: &str,
    now: std::time::Instant,
) -> BarPlan {
    let mut plan = BarPlan::default();

    // Two no-op guards, both load-bearing. Without an observation we know
    // nothing, and acting on nothing is how a transient IPC failure used to
    // close every bar. An empty `expected` is the same story from the other
    // direction: "we could not read the monitor list" must never be actioned
    // as "there are no monitors".
    let (Some(observed), false) = (observed, expected.is_empty()) else {
        return plan;
    };

    // Where each namespace actually is, according to the compositor.
    let mut location: HashMap<&str, &str> = HashMap::new();
    for (monitor, namespaces) in observed {
        for namespace in namespaces {
            location.insert(namespace.as_str(), monitor.as_str());
        }
    }

    // Walk records in id order so the plan does not depend on HashMap order.
    let mut records: Vec<(&window::Id, &BarRecord)> = tracked.iter().collect();
    records.sort_by_key(|(id, _)| **id);

    // Monitors that end this pass with a bar we trust to be there.
    let mut covered: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for (id, record) in records {
        let wanted_gone = !expected.contains(&record.monitor);
        match location.get(record.namespace.as_str()) {
            // Observed exactly where we asked.
            Some(actual) if *actual == record.monitor => {
                if wanted_gone {
                    plan.close.push((*id, CloseReason::MonitorDisconnected));
                } else if covered.insert(actual) {
                    plan.verified.push(*id);
                } else {
                    // Another bar already holds this monitor. Duplicates are
                    // resolved by id order so the choice is stable.
                    plan.close.push((*id, CloseReason::DuplicateOnMonitor));
                }
            }
            // Observed somewhere else: `OutputName` fell back to the focused
            // output and nothing told us. This is the flagship bug, and the
            // only reason it is fixable is that we can see it here.
            Some(_) => plan.close.push((*id, CloseReason::WrongMonitor)),
            // Not mapped anywhere.
            None => {
                if wanted_gone {
                    plan.forget.push((*id, ForgetReason::MonitorDisconnected));
                } else if record.verified {
                    // It was there and is not any more: the surface died
                    // without a usable `Closed` event, which is precisely the
                    // lost-close case that used to strand a monitor forever.
                    plan.forget.push((*id, ForgetReason::SurfaceVanished));
                } else if now.duration_since(record.spawned_at) >= VERIFY_GRACE {
                    // Out of patience — but *close* it rather than forget it.
                    // A surface that has not mapped yet is not a surface that
                    // is gone: it is very much alive, still on its way, and
                    // dropping the record here is what produced bars nothing
                    // could ever reach. This is the flagship bug's second half.
                    plan.close.push((*id, CloseReason::NeverAppeared));
                } else {
                    // Still mapping. Hold its monitor so we do not spawn a
                    // second bar on top of a surface that is on its way.
                    plan.pending.push(*id);
                    covered.insert(record.monitor.as_str());
                }
            }
        }
    }

    // Closing surfaces: wait for the compositor to actually drop them. They
    // deliberately do not count as covering a monitor — a bar on its way out
    // is not a bar — so a replacement is spawned without waiting for the close.
    let mut closing_records: Vec<(&window::Id, &ClosingRecord)> = closing.iter().collect();
    closing_records.sort_by_key(|(id, _)| **id);
    for (id, record) in closing_records {
        if location.contains_key(record.namespace.as_str()) {
            plan.closing_observed.push(*id);
        } else {
            plan.closing_gone.push(*id);
        }
    }

    // Anything of ours on screen that neither set claims. With both halves of
    // the fix in place this should be unreachable, which is the point of
    // reporting it: it is the signature of a surface escaping tracking.
    let known: std::collections::HashSet<&str> = tracked
        .values()
        .map(|r| r.namespace.as_str())
        .chain(closing.values().map(|r| r.namespace.as_str()))
        .collect();
    plan.orphans = location
        .keys()
        .filter(|ns| ns.starts_with(prefix) && !known.contains(*ns))
        .map(|ns| (*ns).to_string())
        .collect();
    plan.orphans.sort();

    // Per-monitor state belongs to monitors that no longer keep a bar.
    let mut dropped: Vec<String> = tracked
        .values()
        .map(|r| r.monitor.clone())
        .filter(|m| !covered.contains(m.as_str()))
        .collect();
    dropped.sort();
    dropped.dedup();
    plan.drop_state_for = dropped;

    // One uncovered monitor per pass, lowest name first for determinism.
    let mut uncovered: Vec<&String> = expected
        .iter()
        .filter(|m| !covered.contains(m.as_str()))
        .collect();
    uncovered.sort();
    plan.spawn = uncovered.first().map(|m| (*m).clone());

    plan.close.sort();
    plan.forget.sort();
    plan.verified.sort();
    plan.pending.sort();
    plan.closing_observed.sort();
    plan.closing_gone.sort();
    plan
}

/// Whether a close that has not taken effect yet should be re-requested.
///
/// Thinned out to powers of two rather than repeated every pass: the first
/// request is the one that matters, and a surface that ignores it is not going
/// to be talked round by a request every 250ms.
pub const fn should_reissue_close(attempts: u32) -> bool {
    attempts.is_power_of_two()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod reconcile_tests {
    use super::{
        plan_from_observation, should_reissue_close, BarPlan, BarRecord, CloseReason,
        ClosingRecord, ForgetReason, VERIFY_GRACE,
    };
    use iced::window;
    use std::collections::{HashMap, HashSet};
    use std::time::Instant;

    /// Stands in for the per-instance prefix; the tests name their surfaces
    /// with it so a name from another instance stays distinguishable.
    const PREFIX: &str = "obayebar-bar-";

    fn expected<const N: usize>(monitors: [&str; N]) -> HashSet<String> {
        monitors.iter().map(|m| (*m).to_string()).collect()
    }

    /// A tracking map from `(monitor, namespace, verified)` triples. Records
    /// start with their full grace window ahead of them.
    fn tracked<const N: usize>(
        entries: [(window::Id, &str, &str, bool); N],
    ) -> HashMap<window::Id, BarRecord> {
        entries
            .into_iter()
            .map(|(id, monitor, namespace, verified)| {
                (
                    id,
                    BarRecord {
                        monitor: monitor.to_string(),
                        namespace: namespace.to_string(),
                        verified,
                        spawned_at: Instant::now(),
                    },
                )
            })
            .collect()
    }

    /// A closing set from `(id, namespace)` pairs.
    fn closing<const N: usize>(
        entries: [(window::Id, &str, u32); N],
    ) -> HashMap<window::Id, ClosingRecord> {
        entries
            .into_iter()
            .map(|(id, namespace, attempts)| {
                (
                    id,
                    ClosingRecord {
                        namespace: namespace.to_string(),
                        attempts,
                    },
                )
            })
            .collect()
    }

    /// The common case: nothing being closed, every grace window still open.
    fn plan(
        observation: Option<&obayebar_core::hypr::LayerMap>,
        monitors: &HashSet<String>,
        bars: &HashMap<window::Id, BarRecord>,
    ) -> BarPlan {
        plan_from_observation(
            observation,
            monitors,
            bars,
            &HashMap::new(),
            PREFIX,
            Instant::now(),
        )
    }

    /// A `j/layers`-shaped observation.
    fn observed<const N: usize>(entries: [(&str, &[&str]); N]) -> obayebar_core::hypr::LayerMap {
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

    #[test]
    fn a_failed_observation_changes_nothing() {
        // The single most damaging old behaviour: an IPC failure read as "no
        // monitors" closed every bar, which under StartMode::Active emptied
        // `units` and killed the process.
        let plan = plan(None, &expected(["DP-1"]), &HashMap::new());
        assert_eq!(plan, BarPlan::default());
    }

    #[test]
    fn an_empty_monitor_set_changes_nothing() {
        let a = window::Id::unique();
        let plan = plan(
            Some(&observed([("DP-1", &["obayebar-bar-1"][..])])),
            &HashSet::new(),
            &tracked([(a, "DP-1", "obayebar-bar-1", true)]),
        );
        assert_eq!(plan, BarPlan::default());
    }

    #[test]
    fn an_empty_setup_spawns_for_one_monitor() {
        let plan = plan(Some(&observed([])), &expected(["DP-1"]), &HashMap::new());
        assert_eq!(plan.spawn.as_deref(), Some("DP-1"));
        assert!(plan.close.is_empty(), "{:?}", plan.close);
    }

    #[test]
    fn spawns_are_serialised_one_per_pass() {
        // Batching them is what made a cold output-name cache stack every bar
        // on the focused monitor: they all resolved against one frozen
        // snapshot inside a context that cannot dispatch the wayland queue.
        let plan = plan(
            Some(&observed([])),
            &expected(["DP-1", "DP-2", "HDMI-A-1"]),
            &HashMap::new(),
        );
        assert_eq!(plan.spawn.as_deref(), Some("DP-1"));
    }

    #[test]
    fn a_bar_observed_where_requested_is_verified() {
        let a = window::Id::unique();
        let plan = plan(
            Some(&observed([("DP-1", &["obayebar-bar-1"][..])])),
            &expected(["DP-1"]),
            &tracked([(a, "DP-1", "obayebar-bar-1", false)]),
        );
        assert_eq!(plan.verified, vec![a]);
        assert_eq!(plan.spawn, None);
        assert!(plan.close.is_empty(), "{:?}", plan.close);
    }

    #[test]
    fn a_bar_on_the_wrong_monitor_is_closed_and_respawned() {
        // The OutputName silent fallback. Previously invisible and permanent:
        // the app kept believing the bar was on DP-2 forever.
        let a = window::Id::unique();
        let plan = plan(
            Some(&observed([("DP-1", &["obayebar-bar-1"][..])])),
            &expected(["DP-1", "DP-2"]),
            &tracked([(a, "DP-2", "obayebar-bar-1", false)]),
        );
        assert_eq!(plan.close, vec![(a, CloseReason::WrongMonitor)]);
        // DP-1 has no bar of ours that we asked for, DP-2 lost its only
        // candidate — one of them gets this pass.
        assert!(plan.spawn.is_some());
    }

    #[test]
    fn two_bars_on_one_monitor_leaves_exactly_one() {
        let a = window::Id::unique();
        let b = window::Id::unique();
        let plan = plan(
            Some(&observed([(
                "DP-1",
                &["obayebar-bar-1", "obayebar-bar-2"][..],
            )])),
            &expected(["DP-1"]),
            &tracked([
                (a, "DP-1", "obayebar-bar-1", true),
                (b, "DP-1", "obayebar-bar-2", true),
            ]),
        );
        let (kept, dropped) = if a < b { (a, b) } else { (b, a) };
        assert_eq!(plan.verified, vec![kept]);
        assert_eq!(plan.close, vec![(dropped, CloseReason::DuplicateOnMonitor)]);
        assert_eq!(plan.spawn, None);
    }

    #[test]
    fn a_disconnected_monitor_closes_its_observed_bar() {
        let a = window::Id::unique();
        let plan = plan(
            Some(&observed([("DP-2", &["obayebar-bar-1"][..])])),
            &expected(["DP-1"]),
            &tracked([(a, "DP-2", "obayebar-bar-1", true)]),
        );
        assert_eq!(plan.close, vec![(a, CloseReason::MonitorDisconnected)]);
        assert_eq!(plan.drop_state_for, vec!["DP-2".to_string()]);
        assert_eq!(plan.spawn.as_deref(), Some("DP-1"));
    }

    #[test]
    fn a_disconnected_monitor_forgets_its_unmapped_bar() {
        let a = window::Id::unique();
        let plan = plan(
            Some(&observed([])),
            &expected(["DP-1"]),
            &tracked([(a, "DP-2", "obayebar-bar-1", true)]),
        );
        assert_eq!(plan.forget, vec![(a, ForgetReason::MonitorDisconnected)]);
        assert!(plan.close.is_empty(), "{:?}", plan.close);
    }

    #[test]
    fn a_vanished_verified_bar_is_forgotten_and_respawned() {
        // The lost-Closed case: layershellev removes the unit before
        // dispatching Closed, so the app never hears about it. Observation is
        // what catches it; without this the monitor stayed masked forever.
        let a = window::Id::unique();
        let plan = plan(
            Some(&observed([("DP-1", &[][..])])),
            &expected(["DP-1"]),
            &tracked([(a, "DP-1", "obayebar-bar-1", true)]),
        );
        assert_eq!(plan.forget, vec![(a, ForgetReason::SurfaceVanished)]);
        assert_eq!(plan.spawn.as_deref(), Some("DP-1"));
        assert_eq!(plan.drop_state_for, vec!["DP-1".to_string()]);
    }

    #[test]
    fn a_freshly_spawned_bar_gets_grace_and_holds_its_monitor() {
        // Verifying immediately after a spawn sees nothing, so an unverified
        // record must not be read as failure — otherwise every spawn is
        // instantly replaced and the bar never settles.
        let a = window::Id::unique();
        let plan = plan(
            Some(&observed([])),
            &expected(["DP-1"]),
            &tracked([(a, "DP-1", "obayebar-bar-1", false)]),
        );
        assert_eq!(plan.pending, vec![a]);
        assert_eq!(plan.spawn, None, "must not double-spawn while mapping");
        assert!(plan.forget.is_empty(), "{:?}", plan.forget);
    }

    #[test]
    fn a_bar_that_never_appears_is_closed_not_merely_forgotten() {
        // The bug that put three bars on one screen after a dock hotplug.
        // Giving up used to drop the record and nothing else, so when the
        // surface finally mapped — a slow compositor, not a dead spawn — it
        // belonged to no one: unclosable, and invisible to every later pass.
        let a = window::Id::unique();
        let now = Instant::now();
        let long_ago = now
            .checked_sub(VERIFY_GRACE)
            .expect("the clock has been running at least as long as the grace window");
        let mut map = tracked([(a, "DP-1", "obayebar-bar-1", false)]);
        map.entry(a).and_modify(|r| r.spawned_at = long_ago);
        let plan = plan_from_observation(
            Some(&observed([])),
            &expected(["DP-1"]),
            &map,
            &closing([]),
            PREFIX,
            now,
        );
        assert_eq!(plan.close, vec![(a, CloseReason::NeverAppeared)]);
        assert_eq!(plan.forget, vec![]);
        assert_eq!(plan.spawn.as_deref(), Some("DP-1"));
    }

    #[test]
    fn the_grace_window_is_wall_clock_not_a_pass_count() {
        // Passes are scheduled by monitor-set changes, and a hotplug emits a
        // burst of those. Counting passes let the burst spend a whole grace
        // window in a fraction of a second, condemning bars that were merely
        // slow to map. Many passes inside the window must change nothing.
        let a = window::Id::unique();
        let map = tracked([(a, "DP-1", "obayebar-bar-1", false)]);
        for _ in 0..50 {
            let plan = plan(Some(&observed([])), &expected(["DP-1"]), &map);
            assert_eq!(plan.pending, vec![a]);
            assert!(plan.close.is_empty(), "{:?}", plan.close);
        }
    }

    #[test]
    fn a_closing_surface_is_watched_until_the_compositor_drops_it() {
        let a = window::Id::unique();
        let still_there = plan_from_observation(
            Some(&observed([("DP-1", &["obayebar-bar-1"][..])])),
            &expected(["DP-1"]),
            &HashMap::new(),
            &closing([(a, "obayebar-bar-1", 0)]),
            PREFIX,
            Instant::now(),
        );
        assert_eq!(still_there.closing_observed, vec![a]);
        assert_eq!(still_there.closing_gone, vec![]);
        // It does not hold DP-1: a bar on its way out is not a bar.
        assert_eq!(still_there.spawn.as_deref(), Some("DP-1"));

        let gone = plan_from_observation(
            Some(&observed([("DP-1", &[][..])])),
            &expected(["DP-1"]),
            &HashMap::new(),
            &closing([(a, "obayebar-bar-1", 3)]),
            PREFIX,
            Instant::now(),
        );
        assert_eq!(gone.closing_gone, vec![a]);
        assert_eq!(gone.closing_observed, vec![]);
    }

    #[test]
    fn a_closing_surface_is_not_reported_as_an_orphan() {
        // Otherwise every ordinary close would raise the alarm it exists for.
        let a = window::Id::unique();
        let plan = plan_from_observation(
            Some(&observed([("DP-1", &["obayebar-bar-1"][..])])),
            &expected(["DP-1"]),
            &HashMap::new(),
            &closing([(a, "obayebar-bar-1", 0)]),
            PREFIX,
            Instant::now(),
        );
        assert_eq!(plan.orphans, Vec::<String>::new());
    }

    #[test]
    fn a_bar_surface_belonging_to_no_record_is_reported() {
        // The shape of the bug, as seen from the outside: a bar on screen that
        // nothing tracks. Unreachable now, and worth an error if it ever
        // happens again.
        let plan = plan(
            Some(&observed([(
                "DP-1",
                &["obayebar-bar-1", "obayebar-panel-audio", "waybar"][..],
            )])),
            &expected(["DP-1"]),
            &HashMap::new(),
        );
        assert_eq!(plan.orphans, vec!["obayebar-bar-1".to_string()]);
    }

    #[test]
    fn another_instances_bars_are_neither_ours_nor_orphans() {
        // A second obayebar started by hand shares the family prefix. Its
        // surfaces must not read as ours gone astray — and, since the
        // generation counter restarts at zero in every process, must not be
        // matched by name either. The pid in the prefix is what separates them.
        let plan = plan_from_observation(
            Some(&observed([("DP-1", &["obayebar-bar-999-1"][..])])),
            &expected(["DP-1"]),
            &HashMap::new(),
            &closing([]),
            "obayebar-bar-1000-",
            Instant::now(),
        );
        assert_eq!(plan.orphans, Vec::<String>::new());
        // Their bar is not ours, so DP-1 still needs one of our own.
        assert_eq!(plan.spawn.as_deref(), Some("DP-1"));
    }

    #[test]
    fn close_requests_are_repeated_but_thinned_out() {
        // The first ask is the one that works; a surface ignoring it will not
        // be won over by one every 250ms for the rest of the session.
        let reissued: Vec<u32> = (0..17).filter(|n| should_reissue_close(*n)).collect();
        assert_eq!(reissued, vec![1, 2, 4, 8, 16]);
    }

    #[test]
    fn foreign_layer_surfaces_are_ignored() {
        // Other clients' layers share j/layers with ours; only our namespaces
        // may influence the plan.
        let a = window::Id::unique();
        let plan = plan(
            Some(&observed([(
                "DP-1",
                &["waybar", "obayebar-bar-1", "gtk-layer-shell"][..],
            )])),
            &expected(["DP-1"]),
            &tracked([(a, "DP-1", "obayebar-bar-1", true)]),
        );
        assert_eq!(plan.verified, vec![a]);
        assert_eq!(plan.spawn, None);
        assert!(plan.close.is_empty(), "{:?}", plan.close);
    }

    #[test]
    fn a_settled_multi_monitor_setup_is_a_no_op() {
        // Idempotence: the steady state must produce an empty plan, or the
        // loop would churn surfaces forever.
        let a = window::Id::unique();
        let b = window::Id::unique();
        let plan = plan(
            Some(&observed([
                ("DP-1", &["obayebar-bar-1"][..]),
                ("DP-2", &["obayebar-bar-2"][..]),
            ])),
            &expected(["DP-1", "DP-2"]),
            &tracked([
                (a, "DP-1", "obayebar-bar-1", true),
                (b, "DP-2", "obayebar-bar-2", true),
            ]),
        );
        assert_eq!(plan.spawn, None);
        assert!(plan.close.is_empty(), "{:?}", plan.close);
        assert!(plan.forget.is_empty(), "{:?}", plan.forget);
        assert!(plan.drop_state_for.is_empty(), "{:?}", plan.drop_state_for);
        assert_eq!(plan.verified.len(), 2);
    }

    #[test]
    fn panel_and_popup_namespaces_never_count_as_bars() {
        // Our own non-bar surfaces are in j/layers too. Counting one as a bar
        // would mask a monitor that has no bar at all.
        let plan = plan(
            Some(&observed([(
                "DP-1",
                &["obayebar-panel-audio", "obayebar-notifications"][..],
            )])),
            &expected(["DP-1"]),
            &HashMap::new(),
        );
        assert_eq!(plan.spawn.as_deref(), Some("DP-1"));
    }

    #[test]
    fn rapid_add_remove_add_converges() {
        // DP-2 disappears and comes back while its bar was still unverified.
        // The stale record must not mask the returning monitor.
        let a = window::Id::unique();
        let map = tracked([(a, "DP-2", "obayebar-bar-1", false)]);

        // Gone: forget it, and DP-1 is the only monitor left to serve.
        let gone = plan(Some(&observed([])), &expected(["DP-1"]), &map);
        assert_eq!(gone.forget, vec![(a, ForgetReason::MonitorDisconnected)]);

        // Back, with nothing tracked: it gets a fresh spawn.
        let back = plan(
            Some(&observed([("DP-1", &["obayebar-bar-2"][..])])),
            &expected(["DP-1", "DP-2"]),
            &HashMap::new(),
        );
        assert_eq!(back.spawn.as_deref(), Some("DP-1"));
    }
}
