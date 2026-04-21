use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DesktopEntry {
    pub name: String,
    pub exec: String,
    pub icon: Option<String>,
    pub comment: Option<String>,
    /// Pre-computed lowercase text for fuzzy matching (name + comment + keywords).
    pub search_text: String,
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

            if let Some(parsed) = parse_desktop_file(&path) {
                seen.insert(filename, parsed);
            }
        }
    }

    let mut entries: Vec<DesktopEntry> = seen.into_values().collect();
    entries.sort_by_key(|e| e.name.to_lowercase());
    entries
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
fn parse_desktop_file(path: &Path) -> Option<DesktopEntry> {
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
        name,
        exec,
        icon,
        comment,
        search_text,
    })
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

        let entry = parse_desktop_file(&path).expect("should parse");
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

        assert!(parse_desktop_file(&path).is_none());

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

        assert!(parse_desktop_file(&path).is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn launch_invalid_command() {
        let result = launch("");
        assert!(result.is_err());
    }
}
