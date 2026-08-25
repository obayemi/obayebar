//! Deciding which picture goes on which monitor, and when.
//!
//! The selection itself is `obayebar_core::wallpaper::plan`, which is pure and
//! tested there. What this adds is the policy around it: startup restores
//! rather than reshuffles, a newly-arrived monitor is filled without disturbing
//! the others, and only a real rotation moves every screen at once.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use obayebar_core::hypr::MonitorInfo;
use obayebar_core::wallpaper::{plan, state};

/// What a pass decided, ready to hand to the renderer and to disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    /// Port name -> picture. Port names are what the renderer knows outputs by.
    pub by_port: BTreeMap<String, PathBuf>,
    /// The state to persist: keyed by the stable panel description, plus the
    /// order and cursor that make "next" meaningful after a restart.
    pub state: state::State,
}

/// Why a pass produced nothing.
///
/// Note there is no "nothing changed" variant. `decide` always reports what
/// every monitor *should* show, even when that matches what it already shows —
/// a fresh process has surfaces with nothing drawn on them, so a startup pass
/// that skipped the assignment because the state file already agreed would
/// leave the screen blank. Whether anything needs redrawing is the renderer's
/// judgement, and whether the state file needs rewriting is the caller's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Idle {
    /// No usable monitors — mirrors and disabled outputs do not count.
    NoMonitors,
    /// The directory held no image any compiled-in decoder understands.
    NoWallpapers,
}

/// Why this pass is running, which is what decides how much it may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Process start, or a re-scan. Restore what each monitor had; only fill
    /// monitors the state has never seen.
    Restore,
    /// A monitor appeared. Same rule — the others must not flicker just
    /// because a screen was plugged in.
    Hotplug,
    /// The timer fired, or the user asked. Every monitor changes together.
    Rotate,
}

/// Work out what every monitor should be showing.
///
/// `order` is the shuffled pool. When the previous state's order no longer
/// matches the directory (files added or removed) it is rebuilt, which is the
/// only time the cursor resets.
pub fn decide(
    monitors: &[MonitorInfo],
    available: &[PathBuf],
    previous: &state::State,
    trigger: Trigger,
    seed: u64,
) -> Result<Assignment, Idle> {
    let usable: Vec<&MonitorInfo> = monitors.iter().filter(|m| m.is_usable()).collect();
    if usable.is_empty() {
        return Err(Idle::NoMonitors);
    }
    if available.is_empty() {
        return Err(Idle::NoWallpapers);
    }

    // Rebuild the order when the pool has changed under us. Comparing sorted
    // contents rather than the sequence: the stored order is a shuffle of the
    // same set, so only membership matters.
    let mut stored: Vec<&PathBuf> = previous.order.iter().collect();
    stored.sort();
    let mut current: Vec<&PathBuf> = available.iter().collect();
    current.sort();
    let (order, mut cursor) = if stored == current && !previous.order.is_empty() {
        (previous.order.clone(), previous.cursor)
    } else {
        log::info!(
            "wallpaper: pool changed, reshuffling {} images",
            available.len()
        );
        (plan::build_order(available, seed), 0)
    };

    // Which monitors need a decision. A restore or a hotplug only fills the
    // ones with nothing remembered; a rotation moves everything.
    let existing: HashMap<String, PathBuf> = previous.previous();
    let needs: Vec<String> = usable
        .iter()
        .filter(|m| match trigger {
            Trigger::Rotate => true,
            Trigger::Restore | Trigger::Hotplug => {
                !existing.contains_key(&m.stable_key())
                    || existing
                        .get(&m.stable_key())
                        .is_some_and(|p| !available.contains(p))
            }
        })
        .map(|m| m.stable_key())
        .collect();

    let mut chosen: BTreeMap<String, PathBuf> = usable
        .iter()
        .filter_map(|m| {
            let key = m.stable_key();
            // Keep what a monitor already has, unless this pass is meant to
            // change it or the file has since vanished from the directory.
            existing
                .get(&key)
                .filter(|p| trigger != Trigger::Rotate && available.contains(p))
                .map(|p| (key, p.clone()))
        })
        .collect();

    if !needs.is_empty() {
        let fresh = plan::plan_wallpapers(&needs, &order, cursor, &existing);
        cursor = fresh.cursor;
        for (key, path) in fresh.assign {
            chosen.insert(key, path);
        }
    }

    // Map back to port names, which is what the renderer addresses outputs by.
    let by_port: BTreeMap<String, PathBuf> = usable
        .iter()
        .filter_map(|m| {
            chosen
                .get(&m.stable_key())
                .map(|p| (m.name.clone(), p.clone()))
        })
        .collect();

    if by_port.is_empty() {
        return Err(Idle::NoWallpapers);
    }

    Ok(Assignment {
        by_port,
        state: state::State {
            version: state::VERSION,
            monitors: chosen,
            order,
            cursor,
        },
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::path::Path;

    use super::*;

    fn monitor(name: &str, description: &str) -> MonitorInfo {
        MonitorInfo {
            name: name.to_string(),
            description: description.to_string(),
            mirror_of: "none".to_string(),
            ..MonitorInfo::default()
        }
    }

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    fn state_with(monitors: &[(&str, &str)], order: &[&str], cursor: usize) -> state::State {
        state::State {
            version: state::VERSION,
            monitors: monitors
                .iter()
                .map(|(k, v)| ((*k).to_string(), PathBuf::from(v)))
                .collect(),
            order: paths(order),
            cursor,
        }
    }

    #[test]
    fn no_usable_monitors_is_idle() {
        let mut mirrored = monitor("DP-2", "B");
        mirrored.mirror_of = "DP-1".to_string();
        let mut off = monitor("DP-3", "C");
        off.disabled = true;
        assert_eq!(
            decide(
                &[mirrored, off],
                &paths(&["a"]),
                &state::State::default(),
                Trigger::Restore,
                1
            ),
            Err(Idle::NoMonitors)
        );
    }

    #[test]
    fn no_wallpapers_is_idle() {
        assert_eq!(
            decide(
                &[monitor("DP-1", "A")],
                &[],
                &state::State::default(),
                Trigger::Restore,
                1
            ),
            Err(Idle::NoWallpapers)
        );
    }

    #[test]
    fn first_run_assigns_every_monitor() {
        let out = decide(
            &[monitor("DP-1", "A"), monitor("DP-2", "B")],
            &paths(&["x", "y", "z"]),
            &state::State::default(),
            Trigger::Restore,
            7,
        )
        .unwrap();
        assert_eq!(out.by_port.len(), 2);
        assert!(out.by_port.contains_key("DP-1"));
        assert!(out.by_port.contains_key("DP-2"));
        // Keyed by description on disk, by port for the renderer.
        assert!(out.state.monitors.contains_key("A"));
    }

    #[test]
    fn restore_keeps_what_each_monitor_had() {
        let previous = state_with(&[("A", "x"), ("B", "y")], &["x", "y", "z"], 2);
        let out = decide(
            &[monitor("DP-1", "A"), monitor("DP-2", "B")],
            &paths(&["x", "y", "z"]),
            &previous,
            Trigger::Restore,
            7,
        )
        .unwrap();
        // Each monitor keeps exactly what it had — but it is still reported,
        // because a freshly started process has drawn nothing yet and needs to
        // be told what to put on screen.
        assert_eq!(
            out.by_port.get("DP-1").map(PathBuf::as_path),
            Some(Path::new("x"))
        );
        assert_eq!(
            out.by_port.get("DP-2").map(PathBuf::as_path),
            Some(Path::new("y"))
        );
        assert_eq!(
            out.state.cursor, previous.cursor,
            "a restore consumes nothing"
        );
    }

    #[test]
    fn restore_survives_a_port_reshuffle() {
        // The same two panels come back on swapped ports, which is what DPMS
        // does. Keying on description is what makes this a no-op.
        let previous = state_with(&[("A", "x"), ("B", "y")], &["x", "y", "z"], 2);
        let out = decide(
            &[monitor("DP-2", "A"), monitor("DP-1", "B")],
            &paths(&["x", "y", "z"]),
            &previous,
            Trigger::Restore,
            7,
        )
        .unwrap();
        // DP-2 now carries panel A, and gets back A's wallpaper.
        assert_eq!(
            out.by_port.get("DP-2").map(PathBuf::as_path),
            Some(Path::new("x"))
        );
        assert_eq!(
            out.by_port.get("DP-1").map(PathBuf::as_path),
            Some(Path::new("y"))
        );
        assert_eq!(out.state.cursor, previous.cursor, "nothing was consumed");
    }

    #[test]
    fn hotplug_fills_only_the_new_monitor() {
        let previous = state_with(&[("A", "x")], &["x", "y", "z"], 1);
        let out = decide(
            &[monitor("DP-1", "A"), monitor("DP-2", "B")],
            &paths(&["x", "y", "z"]),
            &previous,
            Trigger::Hotplug,
            7,
        )
        .unwrap();
        assert_eq!(
            out.by_port.get("DP-1").map(PathBuf::as_path),
            Some(std::path::Path::new("x")),
            "the existing monitor must not flicker"
        );
        assert!(out.by_port.contains_key("DP-2"));
    }

    #[test]
    fn rotate_changes_every_monitor() {
        let previous = state_with(&[("A", "x"), ("B", "y")], &["x", "y", "z"], 0);
        let out = decide(
            &[monitor("DP-1", "A"), monitor("DP-2", "B")],
            &paths(&["x", "y", "z"]),
            &previous,
            Trigger::Rotate,
            7,
        )
        .unwrap();
        assert_eq!(out.by_port.len(), 2);
        // Neither monitor keeps what it had: plan_wallpapers skips a repeat.
        assert_ne!(
            out.by_port.get("DP-1").map(PathBuf::as_path),
            Some(std::path::Path::new("x"))
        );
        assert_ne!(
            out.by_port.get("DP-2").map(PathBuf::as_path),
            Some(std::path::Path::new("y"))
        );
    }

    #[test]
    fn a_vanished_file_is_replaced_even_on_restore() {
        // The remembered picture was deleted from the directory.
        let previous = state_with(&[("A", "gone")], &["x", "y"], 0);
        let out = decide(
            &[monitor("DP-1", "A")],
            &paths(&["x", "y"]),
            &previous,
            Trigger::Restore,
            7,
        )
        .unwrap();
        let assigned = out.by_port.get("DP-1").unwrap();
        assert!(assigned == std::path::Path::new("x") || assigned == std::path::Path::new("y"));
    }

    #[test]
    fn a_changed_pool_reshuffles_and_resets_the_cursor() {
        let previous = state_with(&[("A", "x")], &["x", "y"], 1);
        let out = decide(
            &[monitor("DP-1", "A")],
            &paths(&["x", "y", "brand-new"]),
            &previous,
            Trigger::Rotate,
            7,
        )
        .unwrap();
        assert_eq!(out.state.order.len(), 3);
        let mut sorted = out.state.order;
        sorted.sort();
        assert_eq!(sorted, paths(&["brand-new", "x", "y"]));
    }

    #[test]
    fn a_reordered_pool_of_the_same_files_is_not_a_change() {
        // Only membership matters; the stored order is itself a shuffle.
        let previous = state_with(&[("A", "y")], &["y", "x"], 1);
        let out = decide(
            &[monitor("DP-1", "A")],
            &paths(&["x", "y"]),
            &previous,
            Trigger::Rotate,
            7,
        )
        .unwrap();
        assert_eq!(
            out.state.order,
            paths(&["y", "x"]),
            "cursor context preserved"
        );
    }

    #[test]
    fn a_monitor_without_a_description_still_gets_a_wallpaper() {
        let out = decide(
            &[monitor("DP-1", "")],
            &paths(&["x"]),
            &state::State::default(),
            Trigger::Restore,
            7,
        )
        .unwrap();
        assert!(out.by_port.contains_key("DP-1"));
        // Falls back to keying on the port name.
        assert!(out.state.monitors.contains_key("DP-1"));
    }
}
