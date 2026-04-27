//! Persistent configuration loaded from `$XDG_CONFIG_HOME/obayebar/config.toml`.
//!
//! The TOML file is optional — if missing or malformed, defaults are used and
//! a warning is logged. CLI flags (parsed in `main.rs`) layer on top via
//! `Config::merge_cli`, with CLI taking precedence over the file.
//!
//! For per-feature env vars (e.g. `OBAYEBAR_GITLAB_URL`), precedence is
//! resolved at the consumer site rather than baked into `Config`, so the env
//! var keeps overriding the file but loses to an explicit CLI flag.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

/// Top-level configuration. Add new feature sub-tables as fields here.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub gitlab: GitlabConfig,
}

/// Settings for the GitLab todos panel.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitlabConfig {
    /// Whether to render the GitLab panel on the bar.
    pub enable: bool,
    /// Base URL of the GitLab instance (e.g. `https://gitlab.example.com`).
    /// `None` falls back to `OBAYEBAR_GITLAB_URL` then `https://gitlab.com`.
    pub url: Option<String>,
}

/// Resolve the obayebar config directory: `$XDG_CONFIG_HOME/obayebar` if set,
/// otherwise `~/.config/obayebar`. Returns `None` when neither var is set.
pub fn config_dir() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("obayebar"));
        }
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config").join("obayebar"))
}

fn config_file_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.toml"))
}

impl Config {
    /// Load the config from disk. Missing file → defaults. Parse error →
    /// warn and fall back to defaults so a typo doesn't take the bar down.
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
        match toml::from_str::<Self>(&content) {
            Ok(cfg) => cfg,
            Err(e) => {
                log::warn!("config: ignoring {} ({e})", path.display());
                Self::default()
            }
        }
    }

    /// Apply CLI overrides for fields with no env-var sibling. URL is *not*
    /// merged here — it's kept on `CliOverrides` so the gitlab service can
    /// preserve env-over-file precedence (CLI > env > file > default).
    #[must_use]
    pub const fn merge_cli(mut self, cli: &CliOverrides) -> Self {
        if let Some(enable) = cli.gitlab_enable {
            self.gitlab.enable = enable;
        }
        self
    }
}

/// Subset of CLI args that influence the resolved config. Kept here (rather
/// than referencing `main`'s `CliArgs` directly) so this module stays
/// dependency-free and unit-testable.
#[derive(Debug, Default, Clone)]
pub struct CliOverrides {
    pub gitlab_enable: Option<bool>,
    pub gitlab_url: Option<String>,
}

static CONFIG: OnceLock<Config> = OnceLock::new();
static CLI: OnceLock<CliOverrides> = OnceLock::new();

/// Install the resolved config and CLI overrides exactly once. Subsequent
/// calls are ignored.
pub fn install(cfg: Config, cli: CliOverrides) {
    let _ = CONFIG.set(cfg);
    let _ = CLI.set(cli);
}

/// Read the resolved config. Returns defaults if `install` was never called
/// (e.g. in unit tests that don't go through `main`).
pub fn config() -> &'static Config {
    static DEFAULT: OnceLock<Config> = OnceLock::new();
    CONFIG
        .get()
        .unwrap_or_else(|| DEFAULT.get_or_init(Config::default))
}

/// Read the raw CLI overrides. Used by feature modules that resolve
/// precedence against env vars (e.g. gitlab host).
pub fn cli() -> &'static CliOverrides {
    static DEFAULT: OnceLock<CliOverrides> = OnceLock::new();
    CLI.get()
        .unwrap_or_else(|| DEFAULT.get_or_init(CliOverrides::default))
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
        // deny_unknown_fields surfaces typos so they don't silently no-op.
        let err = toml::from_str::<Config>("[gitlab]\nenabled = true\n");
        assert!(err.is_err(), "expected typo to be rejected, got {err:?}");
    }

    #[test]
    fn cli_overrides_enable() {
        let cfg = Config::default().merge_cli(&CliOverrides {
            gitlab_enable: Some(true),
            gitlab_url: None,
        });
        assert!(cfg.gitlab.enable);
    }

    #[test]
    fn cli_url_does_not_overwrite_file_field() {
        // URL precedence is resolved at the consumer site (gitlab service)
        // so env vars stay in the chain. merge_cli must leave gitlab.url
        // pointing at the file value.
        let file = parse("[gitlab]\nurl = \"https://from-file\"\n");
        let merged = file.merge_cli(&CliOverrides {
            gitlab_enable: None,
            gitlab_url: Some("https://from-cli".to_string()),
        });
        assert_eq!(merged.gitlab.url.as_deref(), Some("https://from-file"));
    }

    #[test]
    fn cli_absent_keeps_file_values() {
        let file = parse("[gitlab]\nenable = true\nurl = \"https://from-file\"\n");
        let merged = file.merge_cli(&CliOverrides::default());
        assert!(merged.gitlab.enable);
        assert_eq!(merged.gitlab.url.as_deref(), Some("https://from-file"));
    }
}
