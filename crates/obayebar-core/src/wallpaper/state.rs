//! The current wallpaper selection, persisted to disk.
//!
//! Two readers need it. The bar writes it when it rotates; the lock screen
//! reads it so it can show the wallpaper the desktop is actually showing,
//! blurred, instead of re-randomising the way `hyprrandlock.fish` did. Keeping
//! the shuffled order and a cursor here is also what makes "next wallpaper"
//! survive a restart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Bumped when the on-disk shape changes incompatibly. A file with any other
/// version is discarded rather than migrated — it holds a wallpaper choice, so
/// regenerating it costs one shuffle.
///
/// v2 re-keyed `monitors` from the port name to `MonitorInfo::stable_key`.
pub const VERSION: u32 = 2;

/// File name inside the state directory.
pub const FILE_NAME: &str = "wallpapers.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    /// `MonitorInfo::stable_key` -> the wallpaper currently on that monitor.
    ///
    /// Keyed by the panel description rather than the port, because DPMS
    /// cycles reshuffle ports: a wallpaper remembered against `DP-9` would be
    /// lost the moment the same screen came back as `DP-10`, and the lock
    /// screen — which keys on description too — would find nothing. A
    /// `BTreeMap` so the serialized form is stable and diffable.
    pub monitors: BTreeMap<String, PathBuf>,
    /// The shuffled pool, so "next" means the same thing after a restart.
    pub order: Vec<PathBuf>,
    /// Index into `order` for the next pick.
    pub cursor: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: VERSION,
            monitors: BTreeMap::new(),
            order: Vec::new(),
            cursor: 0,
        }
    }
}

impl State {
    /// Whether this state can still be used with the current code.
    #[must_use]
    pub const fn is_current(&self) -> bool {
        self.version == VERSION
    }

    /// The wallpaper on each monitor, in the shape `plan_wallpapers` wants.
    #[must_use]
    pub fn previous(&self) -> std::collections::HashMap<String, PathBuf> {
        self.monitors
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// Serialize to the on-disk form. Pretty-printed: it is small, and a human
/// debugging a wallpaper problem will read it.
#[must_use]
pub fn encode(state: &State) -> String {
    serde_json::to_string_pretty(state).unwrap_or_else(|e| {
        // `State` is plain data with no map keys that can fail to serialize,
        // so this is unreachable in practice; returning an empty object keeps
        // the signature infallible rather than propagating a case that cannot
        // happen.
        log::warn!("wallpaper: could not encode state ({e})");
        "{}".to_string()
    })
}

/// Parse the on-disk form, rejecting anything from a different version.
#[must_use]
pub fn decode(text: &str) -> Option<State> {
    match serde_json::from_str::<State>(text) {
        Ok(state) if state.is_current() => Some(state),
        Ok(state) => {
            log::info!(
                "wallpaper: discarding state version {} (want {VERSION})",
                state.version
            );
            None
        }
        Err(e) => {
            log::warn!("wallpaper: ignoring unreadable state ({e})");
            None
        }
    }
}

/// Read the state file, or `None` if it is absent, stale or unreadable.
#[must_use]
pub fn load(path: &Path) -> Option<State> {
    match std::fs::read_to_string(path) {
        Ok(text) => decode(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            log::warn!("wallpaper: could not read {} ({e})", path.display());
            None
        }
    }
}

/// Write the state file atomically.
///
/// Temp file plus rename rather than a plain write: the lock screen reads this
/// while the rotation timer may be writing it, and a half-written file would
/// mean a lock screen with no background.
pub fn save(path: &Path, state: &State) {
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("wallpaper: could not create {} ({e})", parent.display());
            return;
        }
    }
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, encode(state)) {
        log::warn!("wallpaper: could not write {} ({e})", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        log::warn!("wallpaper: could not replace {} ({e})", path.display());
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Where the state file lives: `$XDG_STATE_HOME`-ish, under the data dir.
///
/// The data dir rather than the cache dir on purpose — the selection should
/// survive a reboot, so the desktop comes back looking as it was left instead
/// of reshuffling on every login.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    crate::xdg::data_dir().map(|d| d.join(FILE_NAME))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn sample() -> State {
        let mut monitors = BTreeMap::new();
        monitors.insert("DP-9".to_string(), PathBuf::from("/w/a.jpg"));
        monitors.insert("eDP-1".to_string(), PathBuf::from("/w/b.png"));
        State {
            version: VERSION,
            monitors,
            order: vec![PathBuf::from("/w/a.jpg"), PathBuf::from("/w/b.png")],
            cursor: 1,
        }
    }

    #[test]
    fn round_trips() {
        let state = sample();
        assert_eq!(decode(&encode(&state)).unwrap(), state);
    }

    #[test]
    fn rejects_a_different_version() {
        let mut state = sample();
        state.version = VERSION.wrapping_add(1);
        assert!(decode(&encode(&state)).is_none());
    }

    #[test]
    fn rejects_garbage_without_panicking() {
        for bad in ["", "{", "null", "[]", "{\"version\":1}", "not json at all"] {
            assert!(decode(bad).is_none(), "{bad:?} should not decode");
        }
    }

    #[test]
    fn previous_exposes_the_monitor_map() {
        let previous = sample().previous();
        assert_eq!(
            previous.get("DP-9").map(PathBuf::as_path),
            Some(Path::new("/w/a.jpg"))
        );
        assert_eq!(previous.len(), 2);
    }

    #[test]
    fn default_is_current_and_empty() {
        let state = State::default();
        assert!(state.is_current());
        assert!(state.monitors.is_empty());
        assert_eq!(state.cursor, 0);
    }
}
