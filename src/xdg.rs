//! XDG base-directory helpers, anchored to the obayebar subdir.

use std::path::PathBuf;

const APP_DIR: &str = "obayebar";

/// `$XDG_CONFIG_HOME/obayebar` or `$HOME/.config/obayebar`.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    resolve("XDG_CONFIG_HOME", ".config")
}

/// `$XDG_CACHE_HOME/obayebar` or `$HOME/.cache/obayebar`.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    resolve("XDG_CACHE_HOME", ".cache")
}

/// `$XDG_DATA_HOME/obayebar` or `$HOME/.local/share/obayebar`.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    resolve("XDG_DATA_HOME", ".local/share")
}

fn resolve(env_var: &str, home_subpath: &str) -> Option<PathBuf> {
    if let Ok(base) = std::env::var(env_var) {
        if !base.is_empty() {
            return Some(PathBuf::from(base).join(APP_DIR));
        }
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(home_subpath).join(APP_DIR))
}

/// `$XDG_RUNTIME_DIR/obayebar`, or `None` when it is unset.
///
/// Deliberately no `$HOME` fallback, unlike the others: callers use this for
/// files that must be mode-700 and torn down at logout (the generated hyprlock
/// config names every wallpaper path). Silently landing those in a
/// world-readable `$HOME` directory would defeat the reason for choosing the
/// runtime dir in the first place.
#[must_use]
pub fn runtime_dir() -> Option<PathBuf> {
    let base = std::env::var("XDG_RUNTIME_DIR").ok()?;
    if base.is_empty() {
        return None;
    }
    Some(PathBuf::from(base).join(APP_DIR))
}
