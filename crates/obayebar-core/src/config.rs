//! The shape of `$XDG_CONFIG_HOME/obayebar/config.toml`.
//!
//! The whole file lives here rather than in each binary, and that is
//! deliberate: `#[serde(deny_unknown_fields)]` means a binary parsing only its
//! own subset would reject the file outright the moment another section
//! appeared. Since [`Config::load`] downgrades a parse failure to a warning and
//! defaults, that would present as a feature silently switching itself off.
//!
//! Policy — which flag beats which, and what a missing value falls back to —
//! stays with each binary. This module only describes what the file may say.

use std::path::PathBuf;

use serde::Deserialize;

use crate::xdg;

/// Default wallpaper directory, matching what the shell scripts used.
const DEFAULT_WALLPAPER_DIR: &str = "~/Images/wallpapers/enabled";
/// Default rotation interval. Parsed by
/// [`crate::wallpaper::plan::parse_interval`], so `"off"` here would disable.
const DEFAULT_INTERVAL: &str = "30m";
/// Default base config for the lock screen.
///
/// A path, never a vendored template: the user's own file carries an
/// `auth { fingerprint { … } }` block, and shipping a copy of the repo's
/// example would silently disable fingerprint unlock.
const DEFAULT_HYPRLOCK_CONF: &str = "~/.config/hypr/hyprlock.conf";

/// File-shaped configuration, deserialized from TOML.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub gitlab: GitlabConfig,
    pub wallpaper: WallpaperConfig,
    pub lock: LockConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitlabConfig {
    pub enable: bool,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WallpaperConfig {
    pub enable: bool,
    /// Where to look for images. `~` is expanded.
    pub directory: Option<String>,
    /// How often to rotate: `"30m"`, `"2h"`, or `"off"`.
    pub interval: Option<String>,
}

impl WallpaperConfig {
    /// The configured directory, tilde-expanded, or the default.
    #[must_use]
    pub fn directory(&self) -> PathBuf {
        let raw = self.directory.as_deref().unwrap_or(DEFAULT_WALLPAPER_DIR);
        xdg::expand_tilde(&PathBuf::from(raw))
    }

    /// The configured interval string, or the default.
    #[must_use]
    pub fn interval(&self) -> &str {
        self.interval.as_deref().unwrap_or(DEFAULT_INTERVAL)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LockConfig {
    pub enable: bool,
    /// Base hyprlock config to extend. `~` is expanded.
    pub config: Option<String>,
    /// Blur applied to the generated per-monitor backgrounds.
    pub blur_passes: Option<u32>,
    pub blur_size: Option<u32>,
}

impl LockConfig {
    /// The base hyprlock config path, tilde-expanded, or the default.
    #[must_use]
    pub fn config_path(&self) -> PathBuf {
        let raw = self.config.as_deref().unwrap_or(DEFAULT_HYPRLOCK_CONF);
        xdg::expand_tilde(&PathBuf::from(raw))
    }

    /// Blur settings for the generated backgrounds.
    #[must_use]
    pub fn blur(&self) -> crate::wallpaper::hyprlock::Blur {
        let default = crate::wallpaper::hyprlock::Blur::default();
        crate::wallpaper::hyprlock::Blur {
            passes: self.blur_passes.unwrap_or(default.passes),
            size: self.blur_size.unwrap_or(default.size),
        }
    }
}

impl Config {
    /// Read and parse the config file, falling back to defaults.
    ///
    /// Every failure downgrades to defaults with a warning rather than
    /// stopping a binary from starting: a typo in the GitLab section should
    /// not prevent the screen from locking.
    #[must_use]
    pub fn load() -> Self {
        let Some(path) = config_file_path() else {
            return Self::default();
        };
        let content = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                log::warn!("config: could not read {}: {e}", path.display());
                return Self::default();
            }
        };
        toml::from_str::<Self>(&content).unwrap_or_else(|e| {
            log::warn!("config: ignoring {} ({e})", path.display());
            Self::default()
        })
    }
}

/// Path to the config file, or `None` when no config dir can be determined.
#[must_use]
pub fn config_file_path() -> Option<PathBuf> {
    xdg::config_dir().map(|d| d.join("config.toml"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Config {
        toml::from_str(s).unwrap_or_else(|e| panic!("parse failed: {e}"))
    }

    #[test]
    fn empty_file_yields_defaults() {
        let cfg = parse("");
        assert!(!cfg.gitlab.enable);
        assert!(cfg.gitlab.url.is_none());
        assert!(!cfg.wallpaper.enable);
        assert!(!cfg.lock.enable);
    }

    #[test]
    fn parses_gitlab_enable_only() {
        let cfg = parse("[gitlab]\nenable = true\n");
        assert!(cfg.gitlab.enable);
        assert!(cfg.gitlab.url.is_none());
    }

    #[test]
    fn parses_gitlab_url_only() {
        let cfg = parse("[gitlab]\nurl = \"https://gitlab.example.com\"\n");
        assert!(!cfg.gitlab.enable);
        assert_eq!(
            cfg.gitlab.url.as_deref(),
            Some("https://gitlab.example.com")
        );
    }

    #[test]
    fn unknown_field_is_rejected() {
        let err = toml::from_str::<Config>("[gitlab]\nenabled = true\n");
        assert!(err.is_err(), "expected typo to be rejected, got {err:?}");
    }

    #[test]
    fn unknown_field_is_rejected_in_new_sections_too() {
        // deny_unknown_fields has to keep catching typos as sections are added.
        assert!(toml::from_str::<Config>("[wallpaper]\ndirectry = \"/x\"\n").is_err());
        assert!(toml::from_str::<Config>("[lock]\nblurpasses = 2\n").is_err());
    }

    #[test]
    fn parses_a_full_file_with_every_section() {
        let cfg = parse(
            "[gitlab]\nenable = true\n\n\
             [wallpaper]\nenable = true\ndirectory = \"/pics\"\ninterval = \"45m\"\n\n\
             [lock]\nenable = true\nconfig = \"/etc/hyprlock.conf\"\nblur_passes = 4\nblur_size = 9\n",
        );
        assert!(cfg.gitlab.enable);
        assert!(cfg.wallpaper.enable);
        assert_eq!(cfg.wallpaper.directory(), PathBuf::from("/pics"));
        assert_eq!(cfg.wallpaper.interval(), "45m");
        assert_eq!(cfg.lock.config_path(), PathBuf::from("/etc/hyprlock.conf"));
        assert_eq!(cfg.lock.blur().passes, 4);
        assert_eq!(cfg.lock.blur().size, 9);
    }

    #[test]
    fn omitted_wallpaper_values_fall_back_to_defaults() {
        let cfg = parse("[wallpaper]\nenable = true\n");
        assert_eq!(cfg.wallpaper.interval(), DEFAULT_INTERVAL);
        // The default is a tilde path, so it must come back expanded.
        assert!(!cfg.wallpaper.directory().starts_with("~"));
        assert!(cfg
            .wallpaper
            .directory()
            .ends_with("Images/wallpapers/enabled"));
    }

    #[test]
    fn omitted_lock_values_fall_back_to_defaults() {
        let cfg = parse("[lock]\nenable = true\n");
        assert!(!cfg.lock.config_path().starts_with("~"));
        assert!(cfg
            .lock
            .config_path()
            .ends_with(".config/hypr/hyprlock.conf"));
        assert_eq!(cfg.lock.blur(), crate::wallpaper::hyprlock::Blur::default());
    }

    #[test]
    fn blur_fields_are_independent() {
        let cfg = parse("[lock]\nblur_passes = 7\n");
        let default = crate::wallpaper::hyprlock::Blur::default();
        assert_eq!(cfg.lock.blur().passes, 7);
        assert_eq!(
            cfg.lock.blur().size,
            default.size,
            "setting one blur field must not reset the other"
        );
    }

    #[test]
    fn the_default_interval_parses() {
        // Guards against a default that the parser would reject at runtime.
        assert!(crate::wallpaper::plan::parse_interval(DEFAULT_INTERVAL).is_ok());
    }
}
