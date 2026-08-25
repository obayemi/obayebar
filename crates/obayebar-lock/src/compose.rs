//! Turning live monitors plus the wallpaper state into a hyprlock config.
//!
//! Pure, so the interesting decisions — which monitor gets which picture, what
//! happens when the state is stale or the file is gone — are testable without a
//! compositor or a lock screen.

use std::path::{Path, PathBuf};

use obayebar_core::hypr::MonitorInfo;
use obayebar_core::wallpaper::hyprlock::{self, Blur, LockMonitor, Rendered};
use obayebar_core::wallpaper::state::State;

/// A monitor that will not get its own background, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub monitor: String,
    pub reason: &'static str,
}

/// The generated config plus everything worth telling the user about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composed {
    pub rendered: Rendered,
    pub skipped: Vec<Skipped>,
    /// True when the base config has no monitor-less `background` block.
    ///
    /// Worth a warning rather than a fix: a monitor that wakes up *after* the
    /// lock is taken can only be matched by a block with no monitor set, and
    /// without one its surface renders transparent. The base config belongs to
    /// the user, so we say so instead of editing it.
    pub missing_fallback: bool,
}

/// Whether a remembered file is still usable.
///
/// `render_config` does no existence check, and a path that has since been
/// deleted would produce a background block hyprlock silently ignores — so the
/// monitor would fall through to the base background with no explanation.
fn usable(path: &Path) -> bool {
    path.is_file()
}

/// Build the config for `monitors`, drawing wallpapers from `state`.
///
/// Monitors are matched by `stable_key` — the panel description — because that
/// is what the wallpaper daemon keys its state on, and it survives the port
/// reshuffles DPMS causes.
#[must_use]
pub fn compose(
    base: &str,
    monitors: &[MonitorInfo],
    state: Option<&State>,
    blur: Blur,
) -> Composed {
    let mut lock_monitors = Vec::new();
    let mut skipped = Vec::new();

    for monitor in monitors {
        if !monitor.is_usable() {
            skipped.push(Skipped {
                monitor: monitor.name.clone(),
                reason: "disabled or mirroring another output",
            });
            continue;
        }
        let Some(state) = state else {
            skipped.push(Skipped {
                monitor: monitor.name.clone(),
                reason: "no wallpaper state to read",
            });
            continue;
        };
        let key = monitor.stable_key();
        // Description first, then the port name — a state file written before
        // the daemon knew a description would be keyed the second way.
        let found = state
            .monitors
            .get(&key)
            .or_else(|| state.monitors.get(&monitor.name));
        let Some(path) = found else {
            skipped.push(Skipped {
                monitor: monitor.name.clone(),
                reason: "no wallpaper recorded for this monitor",
            });
            continue;
        };
        if !usable(path) {
            skipped.push(Skipped {
                monitor: monitor.name.clone(),
                reason: "the recorded wallpaper no longer exists",
            });
            continue;
        }
        lock_monitors.push(LockMonitor {
            name: monitor.name.clone(),
            description: monitor.description.clone(),
            wallpaper: path.clone(),
        });
    }

    Composed {
        rendered: hyprlock::render_config(base, &lock_monitors, blur),
        skipped,
        missing_fallback: !hyprlock::has_monitorless_background(base),
    }
}

/// Where to write the generated config.
///
/// The runtime dir, not `/tmp`: it is mode 700 and cleared at logout, so a file
/// naming every wallpaper path is not left world-readable.
#[must_use]
pub fn output_path() -> Option<PathBuf> {
    hyprlock::generated_path()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const BASE: &str =
        "general {\n    hide_cursor = true\n}\n\nbackground {\n    path = /w/fallback.jpg\n}\n";

    fn monitor(name: &str, description: &str) -> MonitorInfo {
        MonitorInfo {
            name: name.to_string(),
            description: description.to_string(),
            mirror_of: "none".to_string(),
            ..MonitorInfo::default()
        }
    }

    /// A real file, since `compose` drops paths that do not exist.
    fn real_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("obayebar-lock-test-files");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, b"x").unwrap();
        path
    }

    fn state_with(entries: &[(&str, PathBuf)]) -> State {
        let mut monitors = BTreeMap::new();
        for (key, path) in entries {
            monitors.insert((*key).to_string(), path.clone());
        }
        State {
            version: obayebar_core::wallpaper::state::VERSION,
            monitors,
            order: Vec::new(),
            cursor: 0,
        }
    }

    #[test]
    fn a_monitor_with_a_wallpaper_gets_a_block() {
        let wall = real_file("a.jpg");
        let state = state_with(&[("Dell TBQ5L", wall.clone())]);
        let out = compose(
            BASE,
            &[monitor("DP-9", "Dell TBQ5L")],
            Some(&state),
            Blur::default(),
        );
        assert!(out.rendered.config.contains("monitor = desc:Dell TBQ5L"));
        assert!(out.rendered.config.contains(&wall.display().to_string()));
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
    }

    #[test]
    fn matching_survives_a_port_reshuffle() {
        // The panel came back on a different port; the description is what the
        // state was keyed on, so the lookup still works.
        let wall = real_file("b.jpg");
        let state = state_with(&[("Dell TBQ5L", wall)]);
        let out = compose(
            BASE,
            &[monitor("DP-10", "Dell TBQ5L")],
            Some(&state),
            Blur::default(),
        );
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        assert!(out.rendered.config.contains("desc:Dell TBQ5L"));
    }

    #[test]
    fn falls_back_to_the_port_name_key() {
        // A state file written for a monitor with no description.
        let wall = real_file("c.jpg");
        let state = state_with(&[("DP-1", wall)]);
        let out = compose(BASE, &[monitor("DP-1", "")], Some(&state), Blur::default());
        assert!(out.skipped.is_empty(), "{:?}", out.skipped);
        assert!(out.rendered.config.contains("monitor = DP-1"));
    }

    #[test]
    fn a_deleted_wallpaper_is_skipped_rather_than_written() {
        // render_config does no existence check, so a stale path would produce
        // a block hyprlock silently ignores.
        let state = state_with(&[("A", PathBuf::from("/does/not/exist.jpg"))]);
        let out = compose(BASE, &[monitor("DP-1", "A")], Some(&state), Blur::default());
        assert_eq!(out.skipped.len(), 1);
        assert_eq!(
            out.skipped.first().map(|s| s.reason),
            Some("the recorded wallpaper no longer exists")
        );
        assert_eq!(
            out.rendered.config.matches("background {").count(),
            1,
            "only the base's own"
        );
    }

    #[test]
    fn an_unknown_monitor_is_skipped() {
        let state = state_with(&[("A", real_file("d.jpg"))]);
        let out = compose(BASE, &[monitor("DP-2", "B")], Some(&state), Blur::default());
        assert_eq!(out.skipped.len(), 1);
        assert_eq!(
            out.skipped.first().map(|s| s.monitor.as_str()),
            Some("DP-2")
        );
    }

    #[test]
    fn no_state_at_all_still_produces_a_usable_config() {
        // First boot, or the wallpaper feature switched off: the lock must
        // still work, using the base config's own background.
        let out = compose(BASE, &[monitor("DP-1", "A")], None, Blur::default());
        assert_eq!(out.skipped.len(), 1);
        assert!(out.rendered.config.starts_with(BASE));
        assert!(!out.missing_fallback);
    }

    #[test]
    fn disabled_and_mirrored_monitors_are_skipped() {
        let mut off = monitor("DP-3", "C");
        off.disabled = true;
        let mut mirror = monitor("DP-4", "D");
        mirror.mirror_of = "DP-1".to_string();
        let out = compose(BASE, &[off, mirror], None, Blur::default());
        assert_eq!(out.skipped.len(), 2);
        assert!(out
            .skipped
            .iter()
            .all(|s| s.reason == "disabled or mirroring another output"));
    }

    #[test]
    fn a_base_without_a_fallback_background_is_flagged() {
        // Not fixed, only reported: a monitor waking mid-lock can only match a
        // monitor-less block, and the base config is the user's.
        let out = compose(
            "general {\n    hide_cursor = true\n}\n",
            &[],
            None,
            Blur::default(),
        );
        assert!(out.missing_fallback);
    }

    #[test]
    fn blur_settings_reach_the_generated_blocks() {
        let wall = real_file("e.jpg");
        let state = state_with(&[("A", wall)]);
        let out = compose(
            BASE,
            &[monitor("DP-1", "A")],
            Some(&state),
            Blur { passes: 4, size: 9 },
        );
        assert!(out.rendered.config.contains("blur_passes = 4"));
        assert!(out.rendered.config.contains("blur_size = 9"));
    }

    #[test]
    fn the_users_own_config_is_never_altered() {
        // Notably the auth/fingerprint block: vendoring or rewriting the base
        // would silently disable fingerprint unlock.
        let base = "auth {\n    fingerprint {\n        enabled = true\n    }\n}\nbackground {\n    path = /w/x.jpg\n}\n";
        let out = compose(base, &[], None, Blur::default());
        assert!(out.rendered.config.starts_with(base));
        assert!(out.rendered.config.contains("fingerprint"));
    }
}
