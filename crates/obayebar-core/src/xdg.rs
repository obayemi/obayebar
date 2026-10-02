//! XDG base-directory helpers, anchored to the obayebar subdir.
//!
//! Resolution itself is [`dirs`]'s job: it ignores a relative
//! `$XDG_*_HOME` per spec, and falls back to the passwd entry when `$HOME`
//! is unset. This module only joins on the app subdir.

use std::path::{Path, PathBuf};

const APP_DIR: &str = "obayebar";

/// `$XDG_CONFIG_HOME/obayebar` or `$HOME/.config/obayebar`.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join(APP_DIR))
}

/// `$XDG_CACHE_HOME/obayebar` or `$HOME/.cache/obayebar`.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join(APP_DIR))
}

/// `$XDG_DATA_HOME/obayebar` or `$HOME/.local/share/obayebar`.
#[must_use]
pub fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(APP_DIR))
}

/// `$XDG_RUNTIME_DIR/obayebar`, or `None` when it is unset or not absolute.
///
/// Deliberately no `$HOME` fallback, unlike the others: callers use this for
/// files that must be mode-700 and torn down at logout (the generated hyprlock
/// config names every wallpaper path). Silently landing those in a
/// world-readable `$HOME` directory would defeat the reason for choosing the
/// runtime dir in the first place. `dirs::runtime_dir` has no HOME fallback
/// either.
#[must_use]
pub fn runtime_dir() -> Option<PathBuf> {
    dirs::runtime_dir().map(|d| d.join(APP_DIR))
}

/// Expand a leading `~` using the user's home directory.
///
/// Config files are hand-written, and `~/Images/wallpapers` is how a person
/// writes that path. Only a leading `~` component expands — a `~` anywhere else
/// is a legitimate filename character and is left alone.
#[must_use]
pub fn expand_tilde(path: &Path) -> PathBuf {
    let Ok(rest) = path.strip_prefix("~") else {
        return path.to_path_buf();
    };
    let Some(home) = dirs::home_dir() else {
        log::warn!(
            "xdg: cannot expand {} because the home directory is unknown",
            path.display()
        );
        return path.to_path_buf();
    };
    home.join(rest)
}

/// `runtime_dir()`, created if needed with mode 700.
///
/// Everything obayebar puts here is private to the session — a control socket,
/// and a generated lock-screen config naming every wallpaper path. Plain
/// `create_dir_all` would apply the umask and typically land on 755, so the
/// mode is set explicitly. Whichever binary gets there first decides, hence one
/// helper rather than a copy in each.
///
/// # Errors
///
/// Returns an [`std::io::Error`] when the directory cannot be created, or
/// `NotFound` when `XDG_RUNTIME_DIR` is unset or not absolute.
pub fn runtime_dir_create() -> std::io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt as _;

    let dir = runtime_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "XDG_RUNTIME_DIR is unset or not absolute",
        )
    })?;
    if !dir.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
    }
    Ok(dir)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn absolute_and_relative_paths_are_untouched() {
        assert_eq!(expand_tilde(Path::new("/etc/x")), PathBuf::from("/etc/x"));
        assert_eq!(expand_tilde(Path::new("a/b")), PathBuf::from("a/b"));
    }

    #[test]
    fn leading_tilde_expands() {
        let expanded = expand_tilde(Path::new("~/a/b"));
        assert!(!expanded.starts_with("~"));
        assert!(expanded.ends_with("a/b"));
    }

    #[test]
    fn tilde_inside_a_path_is_a_normal_character() {
        // "~" is legal in a filename; only the leading component is special.
        assert_eq!(
            expand_tilde(Path::new("/tmp/~backup/x")),
            PathBuf::from("/tmp/~backup/x")
        );
    }
}
