use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesktopEntry {
    /// Unique identifier: the `.desktop` filename (e.g. "firefox.desktop").
    pub desktop_id: String,
    pub name: String,
    pub exec: String,
    pub icon: Option<String>,
    pub comment: Option<String>,
    /// Pre-computed lowercase text for fuzzy matching (name + comment + keywords).
    pub search_text: String,
}

/// Cached launcher data (disposable, rebuilt from desktop entries on cache miss).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LauncherCache {
    pub entries: Vec<DesktopEntry>,
    /// Resolved icon filesystem paths keyed by `desktop_id`.
    pub icon_paths: HashMap<String, PathBuf>,
}

/// Discover and parse all visible `.desktop` application entries from XDG directories.
#[must_use]
pub fn discover_entries() -> Vec<DesktopEntry> {
    let dirs = application_dirs();
    // Deduplicate by filename: later directories override earlier ones per XDG spec.
    let mut seen: HashMap<String, DesktopEntry> = HashMap::new();

    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }

            let Some(filename) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
                continue;
            };

            if let Some(parsed) = parse_desktop_file(&path, &filename) {
                seen.insert(filename, parsed);
            }
        }
    }

    let mut entries: Vec<DesktopEntry> = seen.into_values().collect();
    entries.sort_by_key(|e| e.name.to_lowercase());
    entries
}

/// Resolve icon paths for all entries that have an icon name.
#[must_use]
pub fn resolve_all_icon_paths(entries: &[DesktopEntry]) -> HashMap<String, PathBuf> {
    let mut paths = HashMap::new();
    for entry in entries {
        if let Some(ref icon_name) = entry.icon {
            if let Some(path) = resolve_icon_path(icon_name) {
                paths.insert(entry.desktop_id.clone(), path);
            }
        }
    }
    paths
}

// --- Persistence ---

fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var("XDG_CACHE_HOME")
        .ok()
        .or_else(|| std::env::var("HOME").ok().map(|h| format!("{h}/.cache")))
        .map(PathBuf::from)?;
    Some(base.join("obayebar"))
}

fn data_dir() -> Option<PathBuf> {
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| format!("{h}/.local/share"))
        })
        .map(PathBuf::from)?;
    Some(base.join("obayebar"))
}

/// Directory for pre-resized RGBA icon data.
#[must_use]
pub fn icon_cache_dir() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("icons"))
}

/// Load cached launcher data from disk (disposable cache).
#[must_use]
pub fn load_cache() -> LauncherCache {
    let Some(dir) = cache_dir() else {
        return LauncherCache::default();
    };
    let path = dir.join("launcher.json");
    let Ok(data) = std::fs::read_to_string(&path) else {
        return LauncherCache::default();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

/// Save launcher cache to disk.
pub fn save_cache(cache: &LauncherCache) {
    let Some(dir) = cache_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(data) = serde_json::to_string(cache) else {
        return;
    };
    let path = dir.join("launcher.json");
    if let Err(err) = std::fs::write(&path, data) {
        log::warn!("Failed to write launcher cache: {err}");
    }
}

/// Load launch frequency counts from XDG data directory (persistent user data).
#[must_use]
pub fn load_launch_counts() -> HashMap<String, u32> {
    // Try XDG_DATA_HOME first
    if let Some(dir) = data_dir() {
        let path = dir.join("launch-counts.json");
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(counts) = serde_json::from_str(&data) {
                return counts;
            }
        }
    }

    // Migrate from old cache locations
    if let Some(dir) = cache_dir() {
        // Try old launch-history.json
        let old_path = dir.join("launch-history.json");
        if let Ok(data) = std::fs::read_to_string(&old_path) {
            if let Ok(counts) = serde_json::from_str::<HashMap<String, u32>>(&data) {
                std::fs::remove_file(&old_path).ok();
                return counts;
            }
        }
        // Try old launcher.json that had embedded launch_counts
        let cache_path = dir.join("launcher.json");
        if let Ok(data) = std::fs::read_to_string(&cache_path) {
            #[derive(Deserialize)]
            struct OldCache {
                #[serde(default)]
                launch_counts: HashMap<String, u32>,
            }
            if let Ok(old) = serde_json::from_str::<OldCache>(&data) {
                if !old.launch_counts.is_empty() {
                    return old.launch_counts;
                }
            }
        }
    }

    HashMap::new()
}

/// Save launch frequency counts to XDG data directory.
#[allow(clippy::implicit_hasher)]
pub fn save_launch_counts(counts: &HashMap<String, u32>) {
    let Some(dir) = data_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(data) = serde_json::to_string(counts) else {
        return;
    };
    let path = dir.join("launch-counts.json");
    if let Err(err) = std::fs::write(&path, data) {
        log::warn!("Failed to write launch counts: {err}");
    }
}

/// Collect application directories from `XDG_DATA_DIRS` and common paths.
fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // User-local applications first (highest priority)
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(format!("{home}/.local/share/applications")));
    }

    // XDG_DATA_DIRS
    let xdg_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in xdg_dirs.split(':') {
        if !dir.is_empty() {
            dirs.push(PathBuf::from(format!("{dir}/applications")));
        }
    }

    // NixOS system path
    dirs.push(PathBuf::from("/run/current-system/sw/share/applications"));

    dirs
}

/// Parse a single `.desktop` file. Returns `None` if the entry should be hidden
/// or is not a valid application entry.
fn parse_desktop_file(path: &Path, desktop_id: &str) -> Option<DesktopEntry> {
    let content = std::fs::read_to_string(path).ok()?;

    let mut in_desktop_entry = false;
    let mut name: Option<String> = None;
    let mut exec: Option<String> = None;
    let mut icon: Option<String> = None;
    let mut comment: Option<String> = None;
    let mut keywords: Option<String> = None;
    let mut entry_type: Option<String> = None;
    let mut no_display = false;
    let mut hidden = false;

    for line in content.lines() {
        let line = line.trim();

        if line.starts_with('[') {
            if in_desktop_entry {
                // We've left [Desktop Entry], stop parsing
                break;
            }
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }

        if !in_desktop_entry || line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        match key {
            "Name" => name = Some(value.to_string()),
            "Exec" => exec = Some(sanitize_exec(value)),
            "Icon" => icon = Some(value.to_string()),
            "Comment" => comment = Some(value.to_string()),
            "Keywords" => keywords = Some(value.to_string()),
            "Type" => entry_type = Some(value.to_string()),
            "NoDisplay" => no_display = value.eq_ignore_ascii_case("true"),
            "Hidden" => hidden = value.eq_ignore_ascii_case("true"),
            _ => {}
        }
    }

    // Filter out non-application and hidden entries
    if no_display || hidden {
        return None;
    }
    if entry_type.as_deref().is_some_and(|t| t != "Application") {
        return None;
    }

    let name = name?;
    let exec = exec?;

    // Build search text: lowercase name + comment + keywords
    let mut search_parts = vec![name.to_lowercase()];
    if let Some(ref c) = comment {
        search_parts.push(c.to_lowercase());
    }
    if let Some(ref k) = keywords {
        search_parts.push(k.to_lowercase());
    }
    let search_text = search_parts.join(" ");

    Some(DesktopEntry {
        desktop_id: desktop_id.to_string(),
        name,
        exec,
        icon,
        comment,
        search_text,
    })
}

/// Resolve an icon name to an actual file path by searching XDG icon directories.
///
/// Supports absolute paths, and searches hicolor theme directories and pixmaps
/// for PNG files at standard sizes.
#[must_use]
pub fn resolve_icon_path(icon_name: &str) -> Option<PathBuf> {
    // Absolute path: use directly if it exists
    if icon_name.starts_with('/') {
        let path = PathBuf::from(icon_name);
        return path.exists().then_some(path);
    }

    let sizes = [
        "48x48", "64x64", "32x32", "128x128", "256x256", "24x24", "96x96", "512x512",
    ];
    let extensions = ["png"];

    for dir in &icon_theme_dirs() {
        for size in &sizes {
            for ext in &extensions {
                let path = dir
                    .join(size)
                    .join("apps")
                    .join(format!("{icon_name}.{ext}"));
                if path.exists() {
                    return Some(path);
                }
            }
        }
    }

    // Check pixmaps directories
    let pixmap_dirs = [
        PathBuf::from("/usr/share/pixmaps"),
        PathBuf::from("/run/current-system/sw/share/pixmaps"),
    ];
    for dir in &pixmap_dirs {
        for ext in &extensions {
            let path = dir.join(format!("{icon_name}.{ext}"));
            if path.exists() {
                return Some(path);
            }
        }
    }

    None
}

/// Collect icon theme directories (hicolor) from standard XDG locations.
fn icon_theme_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(format!("{home}/.local/share/icons/hicolor")));
        dirs.push(PathBuf::from(format!("{home}/.icons/hicolor")));
    }

    let xdg_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in xdg_dirs.split(':') {
        if !dir.is_empty() {
            dirs.push(PathBuf::from(format!("{dir}/icons/hicolor")));
        }
    }

    dirs.push(PathBuf::from("/run/current-system/sw/share/icons/hicolor"));

    dirs
}

/// Strip XDG field codes (%f, %F, %u, %U, etc.) from an Exec value.
fn sanitize_exec(exec: &str) -> String {
    exec.split_whitespace()
        .filter(|arg| {
            !matches!(
                *arg,
                "%f" | "%F"
                    | "%u"
                    | "%U"
                    | "%d"
                    | "%D"
                    | "%n"
                    | "%N"
                    | "%i"
                    | "%c"
                    | "%k"
                    | "%v"
                    | "%m"
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Launch an application from its Exec string.
///
/// # Errors
///
/// Returns an error if the command cannot be spawned.
pub fn launch(exec: &str) -> Result<(), std::io::Error> {
    let parts: Vec<&str> = exec.split_whitespace().collect();
    let program = parts.first().ok_or(std::io::ErrorKind::InvalidInput)?;

    std::process::Command::new(program)
        .args(parts.get(1..).unwrap_or_default())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_exec_strips_field_codes() {
        assert_eq!(sanitize_exec("firefox %u"), "firefox");
        assert_eq!(sanitize_exec("code %F --new-window"), "code --new-window");
        assert_eq!(sanitize_exec("app %f %U %i"), "app");
    }

    #[test]
    fn sanitize_exec_preserves_normal_args() {
        assert_eq!(sanitize_exec("myapp --flag value"), "myapp --flag value");
    }

    #[test]
    fn parse_desktop_file_valid() {
        let dir = std::env::temp_dir().join("obayebar_test_desktop");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("test.desktop");
        std::fs::write(
            &path,
            "[Desktop Entry]\nType=Application\nName=Test App\nExec=test-app %u\nComment=A test\nKeywords=testing;demo;\n",
        )
        .ok();

        let entry =
            parse_desktop_file(&path, "test.desktop").unwrap_or_else(|| panic!("should parse"));
        assert_eq!(entry.desktop_id, "test.desktop");
        assert_eq!(entry.name, "Test App");
        assert_eq!(entry.exec, "test-app");
        assert_eq!(entry.comment.as_deref(), Some("A test"));
        assert!(entry.search_text.contains("test app"));
        assert!(entry.search_text.contains("testing;demo;"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_desktop_file_hidden() {
        let dir = std::env::temp_dir().join("obayebar_test_hidden");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("hidden.desktop");
        std::fs::write(
            &path,
            "[Desktop Entry]\nType=Application\nName=Hidden\nExec=hidden\nNoDisplay=true\n",
        )
        .ok();

        assert!(parse_desktop_file(&path, "hidden.desktop").is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_desktop_file_non_application() {
        let dir = std::env::temp_dir().join("obayebar_test_link");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("link.desktop");
        std::fs::write(
            &path,
            "[Desktop Entry]\nType=Link\nName=A Link\nURL=https://example.com\n",
        )
        .ok();

        assert!(parse_desktop_file(&path, "link.desktop").is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn launch_invalid_command() {
        let result = launch("");
        assert!(result.is_err());
    }

    #[test]
    fn cache_round_trip() {
        let cache = LauncherCache {
            entries: vec![DesktopEntry {
                desktop_id: "test.desktop".into(),
                name: "Test".into(),
                exec: "test".into(),
                icon: None,
                comment: None,
                search_text: "test".into(),
            }],
            icon_paths: HashMap::from([("test.desktop".into(), PathBuf::from("/icon.png"))]),
        };
        let json = serde_json::to_string(&cache).unwrap_or_default();
        let loaded: LauncherCache = serde_json::from_str(&json).unwrap_or_default();
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.icon_paths.len(), 1);
    }

    #[test]
    fn launch_counts_round_trip() {
        let counts: HashMap<String, u32> =
            HashMap::from([("firefox.desktop".into(), 42), ("code.desktop".into(), 7)]);
        let json = serde_json::to_string(&counts).unwrap_or_default();
        let loaded: HashMap<String, u32> = serde_json::from_str(&json).unwrap_or_default();
        assert_eq!(loaded.get("firefox.desktop").copied(), Some(42));
        assert_eq!(loaded.get("code.desktop").copied(), Some(7));
    }
}
