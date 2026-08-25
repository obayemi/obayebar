//! XDG base-directory helpers, anchored to the obayebar subdir.

use std::path::{Path, PathBuf};

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

/// Expand a leading `~` using `$HOME`.
///
/// Config files are hand-written, and `~/Images/wallpapers` is how a person
/// writes that path. Only a leading `~` component expands — a `~` anywhere else
/// is a legitimate filename character and is left alone.
#[must_use]
pub fn expand_tilde(path: &Path) -> PathBuf {
    let Ok(rest) = path.strip_prefix("~") else {
        return path.to_path_buf();
    };
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => PathBuf::from(home).join(rest),
        _ => {
            log::warn!(
                "xdg: cannot expand {} because HOME is unset",
                path.display()
            );
            path.to_path_buf()
        }
    }
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
/// `NotFound` when `XDG_RUNTIME_DIR` is unset.
pub fn runtime_dir_create() -> std::io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt as _;

    let dir = runtime_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is unset")
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
