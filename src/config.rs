//! TOML config from `$XDG_CONFIG_HOME/obayebar/config.toml` plus CLI overrides.
//!
//! Per-field precedence: CLI flag > env var (where applicable) > config file > default.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

use obayebar::xdg;

const DEFAULT_GITLAB_HOST: &str = "https://gitlab.com";
const ENV_GITLAB_URL: &str = "OBAYEBAR_GITLAB_URL";

/// File-shaped configuration, deserialized from TOML.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub gitlab: GitlabConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GitlabConfig {
    pub enable: bool,
    pub url: Option<String>,
}

/// CLI flags that override file values.
#[derive(Debug, Default, Clone)]
pub struct CliOverrides {
    pub gitlab_enable: Option<bool>,
    pub gitlab_url: Option<String>,
}

/// File + CLI + env, with each field's precedence resolved at construction.
/// Static, populated once from `main`.
#[derive(Debug, Default, Clone)]
pub struct Resolved {
    gitlab_enable: bool,
    gitlab_host: String,
}

impl Config {
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

impl Resolved {
    #[must_use]
    pub fn from_parts(file: &Config, cli: &CliOverrides) -> Self {
        Self {
            gitlab_enable: cli.gitlab_enable.unwrap_or(file.gitlab.enable),
            gitlab_host: resolve_gitlab_host(file, cli),
        }
    }

    #[must_use]
    pub const fn gitlab_enable(&self) -> bool {
        self.gitlab_enable
    }

    #[must_use]
    pub fn gitlab_host(&self) -> &str {
        &self.gitlab_host
    }
}

fn resolve_gitlab_host(file: &Config, cli: &CliOverrides) -> String {
    if let Some(url) = cli.gitlab_url.as_deref() {
        return normalize_host(url);
    }
    if let Ok(env_url) = std::env::var(ENV_GITLAB_URL) {
        let trimmed = env_url.trim();
        if !trimmed.is_empty() {
            return normalize_host(trimmed);
        }
    }
    if let Some(url) = file.gitlab.url.as_deref() {
        return normalize_host(url);
    }
    DEFAULT_GITLAB_HOST.to_string()
}

fn normalize_host(s: &str) -> String {
    s.trim().trim_end_matches('/').to_string()
}

fn config_file_path() -> Option<PathBuf> {
    xdg::config_dir().map(|d| d.join("config.toml"))
}

static RESOLVED: OnceLock<Resolved> = OnceLock::new();

pub fn install(file: &Config, cli: &CliOverrides) {
    let _ = RESOLVED.set(Resolved::from_parts(file, cli));
}

pub fn resolved() -> &'static Resolved {
    static DEFAULT: OnceLock<Resolved> = OnceLock::new();
    RESOLVED
        .get()
        .unwrap_or_else(|| DEFAULT.get_or_init(Resolved::default))
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
        let err = toml::from_str::<Config>("[gitlab]\nenabled = true\n");
        assert!(err.is_err(), "expected typo to be rejected, got {err:?}");
    }

    #[test]
    fn cli_enable_beats_file() {
        let file = parse("[gitlab]\nenable = false\n");
        let r = Resolved::from_parts(
            &file,
            &CliOverrides {
                gitlab_enable: Some(true),
                gitlab_url: None,
            },
        );
        assert!(r.gitlab_enable());
    }

    #[test]
    fn cli_url_beats_file() {
        let file = parse("[gitlab]\nurl = \"https://from-file\"\n");
        let r = Resolved::from_parts(
            &file,
            &CliOverrides {
                gitlab_enable: None,
                gitlab_url: Some("https://from-cli".to_string()),
            },
        );
        assert_eq!(r.gitlab_host(), "https://from-cli");
    }

    #[test]
    fn file_url_used_when_cli_absent() {
        let file = parse("[gitlab]\nurl = \"https://from-file\"\n");
        let r = Resolved::from_parts(&file, &CliOverrides::default());
        assert_eq!(r.gitlab_host(), "https://from-file");
    }

    #[test]
    fn default_host_when_nothing_set() {
        let r = Resolved::from_parts(&Config::default(), &CliOverrides::default());
        assert_eq!(r.gitlab_host(), "https://gitlab.com");
    }

    #[test]
    fn host_trailing_slash_trimmed() {
        let file = parse("[gitlab]\nurl = \"https://example.com/\"\n");
        let r = Resolved::from_parts(&file, &CliOverrides::default());
        assert_eq!(r.gitlab_host(), "https://example.com");
    }
}
