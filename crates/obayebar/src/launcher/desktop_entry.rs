//! Desktop-entry discovery: what the launcher shows, and how it starts it.
//!
//! Parsing goes through `freedesktop-desktop-entry` and icon lookup through
//! `freedesktop-icons` rather than the hand-rolled versions this file used to
//! carry. Both handle parts of the spec the hand-rolled code did not: localized
//! `Name[fr]`, `OnlyShowIn` / `NotShowIn`, `TryExec`, desktop IDs from nested
//! directories (`kde/foo.desktop` is `kde-foo`), and icon themes with their
//! inheritance — the old lookup searched `hicolor` only, so an app whose icon
//! ships in Adwaita simply had none.
//!
//! What is *not* delegated is the `Exec` line. `parse_exec()` splits on
//! whitespace, which mangles a quoted argument containing spaces; the entries
//! on this machine include exactly that shape, so [`sanitize_exec`] and
//! [`launch`] stay and hand the line to a shell, which is what the spec's
//! quoted-string Exec semantics ask for.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use freedesktop_desktop_entry as fde;

use obayebar_core::xdg::{config_dir, data_dir};

use super::icons::ICON_SIZE;

/// One launchable application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    /// Desktop ID as defined by the spec: the path below `applications/` with
    /// separators turned into dashes and the extension dropped, e.g.
    /// `org.mozilla.firefox` or `kde-systemsettings`.
    pub id: String,
    pub name: String,
    /// Sanitized `Exec` line, ready to hand to a shell.
    pub exec: String,
    pub icon: Option<String>,
    pub comment: Option<String>,
    /// `Terminal=true`: the program is a TUI and needs a terminal emulator
    /// wrapped around it, or it exits immediately against a null tty.
    pub terminal: bool,
    /// Pre-computed lowercase text for fuzzy matching (name + comment +
    /// keywords).
    pub search_text: String,
}

/// Everything one discovery pass produced.
#[derive(Debug, Clone, Default)]
pub struct Index {
    pub entries: Vec<DesktopEntry>,
    /// Resolved icon filesystem paths keyed by [`DesktopEntry::id`].
    pub icon_paths: HashMap<String, PathBuf>,
}

/// NixOS keeps the system profile's entries here. It is normally also in
/// `XDG_DATA_DIRS`, but a systemd user unit can be started with a thinner
/// environment than a login shell, and a bar with no applications in it is a
/// worse failure than one redundant directory.
const NIXOS_APPLICATIONS: &str = "/run/current-system/sw/share/applications";

/// Directories to scan, highest priority first.
///
/// Priority is what decides which of two entries with the same desktop ID wins:
/// per the spec the *first* one found, which is why this order matters and why
/// [`discover`] keeps the first rather than the last.
#[must_use]
pub fn application_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fde::default_paths().collect();
    dirs.push(PathBuf::from(NIXOS_APPLICATIONS));

    let mut seen = HashSet::new();
    dirs.retain(|dir| seen.insert(dir.clone()));
    dirs
}

/// Discover every visible application entry, with its icon resolved.
#[must_use]
pub fn discover() -> Index {
    discover_in(application_dirs())
}

/// [`discover`] over an explicit directory list, highest priority first.
fn discover_in(dirs: Vec<PathBuf>) -> Index {
    let locales = fde::get_languages_from_env();
    let desktops = current_desktops();
    let theme = icon_theme();

    let mut entries: Vec<DesktopEntry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for path in fde::Iter::new(dirs.into_iter()) {
        if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
            continue;
        }
        let Ok(parsed) = fde::DesktopEntry::from_path(&path, Some(&locales)) else {
            continue;
        };
        // First one wins: `Iter` walks the directories in priority order, so a
        // user's ~/.local/share override must not be replaced by the system
        // copy that comes after it. The old code inserted into a map keyed by
        // filename, which kept the *last* — precedence exactly backwards.
        if !seen.insert(parsed.appid.clone()) {
            continue;
        }
        if let Some(entry) = convert(&parsed, &locales, desktops.as_deref()) {
            entries.push(entry);
        }
    }

    entries.sort_by_key(|e| e.name.to_lowercase());

    // One memo per pass, not the crate's global cache: a `NotFound` cached
    // there would outlive the install that fixes it, and this process is a
    // daemon that rescans rather than a command that exits.
    let mut resolved: HashMap<String, Option<PathBuf>> = HashMap::new();
    let mut icon_paths = HashMap::new();
    for entry in &entries {
        let Some(icon) = entry.icon.as_deref() else {
            continue;
        };
        let path = resolved
            .entry(icon.to_string())
            .or_insert_with(|| resolve_icon_path(icon, &theme));
        if let Some(path) = path.clone() {
            icon_paths.insert(entry.id.clone(), path);
        }
    }

    log::info!(
        "launcher: {} entries, {} icons resolved (theme {theme})",
        entries.len(),
        icon_paths.len()
    );

    Index {
        entries,
        icon_paths,
    }
}

/// Turn a parsed entry into ours, or `None` if it should not be listed.
fn convert(
    entry: &fde::DesktopEntry,
    locales: &[String],
    desktops: Option<&[String]>,
) -> Option<DesktopEntry> {
    if entry.type_().is_some_and(|t| t != "Application") {
        return None;
    }
    if entry.no_display() || entry.hidden() {
        return None;
    }
    // `TryExec` is the spec's "is this actually installed" probe. Entries
    // shipped by a package whose binary lives elsewhere (or was removed) name
    // one, and listing them means offering a launch that cannot work.
    if let Some(try_exec) = entry.try_exec() {
        if find_in_path(try_exec).is_none() {
            return None;
        }
    }
    if !shown_in(entry, desktops) {
        return None;
    }

    let name = entry.name(locales)?.into_owned();
    let exec = sanitize_exec(entry.exec()?);
    let comment = entry.comment(locales).map(std::borrow::Cow::into_owned);
    let keywords = entry.keywords(locales).unwrap_or_default();

    let mut search_text = name.to_lowercase();
    if let Some(comment) = comment.as_deref() {
        search_text.push(' ');
        search_text.push_str(&comment.to_lowercase());
    }
    for keyword in &keywords {
        search_text.push(' ');
        search_text.push_str(&keyword.to_lowercase());
    }

    Some(DesktopEntry {
        id: entry.appid.clone(),
        name,
        exec,
        icon: entry.icon().map(ToString::to_string),
        comment,
        terminal: entry.terminal(),
        search_text,
    })
}

/// Apply `OnlyShowIn` / `NotShowIn` against the current desktop.
///
/// With no `XDG_CURRENT_DESKTOP` both keys are ignored rather than treated as
/// "matches nothing". A systemd user unit can be started before the desktop
/// environment sets that variable, and hiding every `OnlyShowIn` entry in that
/// case would silently shrink the list for a reason the user cannot see.
fn shown_in(entry: &fde::DesktopEntry, desktops: Option<&[String]>) -> bool {
    let Some(desktops) = desktops else {
        return true;
    };
    let matches = |list: Vec<&str>| {
        list.iter()
            .any(|name| desktops.iter().any(|d| d.eq_ignore_ascii_case(name)))
    };
    if entry.only_show_in().is_some_and(|list| !matches(list)) {
        return false;
    }
    !entry.not_show_in().is_some_and(matches)
}

/// The desktops named by `XDG_CURRENT_DESKTOP`, lowercased.
fn current_desktops() -> Option<Vec<String>> {
    let desktops = fde::current_desktop()?;
    if desktops.is_empty() {
        return None;
    }
    Some(desktops)
}

/// The icon theme to look in.
///
/// Read from the GTK settings file rather than by shelling out to `gsettings`
/// (which is what `freedesktop-icons` does when asked): that spawns a process
/// and talks to dconf, on a path the bar walks at every rescan. Falling back to
/// `hicolor` is not a loss — lookup ends up there anyway, since the crate
/// follows theme inheritance and then hicolor and pixmaps.
fn icon_theme() -> String {
    const DEFAULT: &str = "hicolor";
    let Some(config) = config_dir().and_then(|d| d.parent().map(Path::to_path_buf)) else {
        return DEFAULT.to_string();
    };
    for version in ["gtk-4.0", "gtk-3.0"] {
        let Ok(text) = std::fs::read_to_string(config.join(version).join("settings.ini")) else {
            continue;
        };
        let found = text.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == "gtk-icon-theme-name").then(|| value.trim().to_string())
        });
        if let Some(theme) = found.filter(|t| !t.is_empty()) {
            return theme;
        }
    }
    DEFAULT.to_string()
}

/// Resolve an icon name to a file, honouring `theme` and its inheritance.
///
/// An absolute path is used as-is: it is not a themed icon name, and the
/// lookup would only search for a file named after the whole path.
#[must_use]
pub fn resolve_icon_path(icon: &str, theme: &str) -> Option<PathBuf> {
    if icon.starts_with('/') {
        let path = PathBuf::from(icon);
        return path.is_file().then_some(path);
    }
    freedesktop_icons::lookup(icon)
        .with_size(u16::try_from(ICON_SIZE).unwrap_or(24))
        .with_theme(theme)
        .find()
}

// --- Launch frequency (persistent user data) ---

/// Load launch frequency counts from the XDG data directory.
#[must_use]
pub fn load_launch_counts() -> HashMap<String, u32> {
    let Some(path) = data_dir().map(|d| d.join("launch-counts.json")) else {
        return HashMap::new();
    };
    let Ok(data) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    serde_json::from_str(&data).unwrap_or_else(|err| {
        log::warn!("launcher: ignoring unreadable launch counts ({err})");
        HashMap::new()
    })
}

/// Save launch frequency counts to the XDG data directory.
#[allow(clippy::implicit_hasher)]
pub fn save_launch_counts(counts: &HashMap<String, u32>) {
    let Some(dir) = data_dir() else {
        return;
    };
    if let Err(err) = std::fs::create_dir_all(&dir) {
        log::warn!("launcher: cannot create {} ({err})", dir.display());
        return;
    }
    let Ok(data) = serde_json::to_string(counts) else {
        return;
    };
    if let Err(err) = std::fs::write(dir.join("launch-counts.json"), data) {
        log::warn!("launcher: failed to write launch counts ({err})");
    }
}

// --- Launching ---

/// Strip XDG field codes from an Exec value, leaving a shell-runnable string.
///
/// Anything `%`-prefixed is a field code: we launch without files or URIs, so
/// none of them have a value to expand to. `%%` is the spec's escape for a
/// literal percent and is unescaped rather than dropped. Flatpak's `@@` /
/// `@@u` file-forwarding markers are not field codes, so the old
/// exact-match filter kept them and flatpak received them as positional
/// arguments; they bracket an argument list we do not supply, so they go too.
///
/// Quoting is deliberately left intact — [`launch`] hands the result to a
/// shell, which is what the spec's quoted-string Exec semantics require.
fn sanitize_exec(exec: &str) -> String {
    exec.split_whitespace()
        .filter_map(|arg| match arg {
            "@@" | "@@u" | "@@U" => None,
            _ => match arg.strip_prefix('%') {
                // Escaped literal percent, not a field code.
                Some("%") => Some("%"),
                // Any other %-token is a field code with nothing to expand to.
                Some(_) => None,
                None => Some(arg),
            },
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Terminal emulators tried in order when `$TERMINAL` is unset, for entries
/// that declare `Terminal=true`.
const TERMINAL_CANDIDATES: [&str; 8] = [
    "foot",
    "kitty",
    "alacritty",
    "wezterm",
    "konsole",
    "gnome-terminal",
    "xfce4-terminal",
    "xterm",
];

/// Resolve `name` against `$PATH`. Used to pick a terminal that actually
/// exists rather than spawning a missing one and reporting success, and to
/// honour `TryExec`.
fn find_in_path(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let path = PathBuf::from(name);
        return path.is_file().then_some(path);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// Build the argv that makes `terminal` run `command`.
///
/// Most emulators take the xterm-compatible `-e`; a few want the command as
/// trailing positionals instead, and wezterm wants a subcommand. `command`
/// always goes through `sh -c` so the Exec line's own quoting survives.
fn terminal_argv(terminal: &str, command: &str) -> Vec<String> {
    let bare = Path::new(terminal)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(terminal);
    let inner = ["sh".to_string(), "-c".to_string(), command.to_string()];
    let prefix: Vec<String> = match bare {
        // Take the command directly as trailing arguments.
        "foot" | "kitty" => vec![terminal.to_string()],
        "wezterm" => vec![terminal.to_string(), "start".to_string(), "--".to_string()],
        _ => vec![terminal.to_string(), "-e".to_string()],
    };
    prefix.into_iter().chain(inner).collect()
}

/// Launch an application from its (already sanitized) Exec string.
///
/// The command runs through `sh -c` rather than being split on whitespace a
/// second time: an Exec line is a quoted string with shell-like word rules, so
/// `sh -c "exec /opt/app --flag"` wrappers, `env VAR=value app` prefixes and
/// quoted arguments all need a shell to come out with the right argv.
///
/// When `terminal` is set the command is wrapped in a terminal emulator —
/// without one it inherits the null stdio below, finds no tty, and exits
/// immediately while `spawn` still reports success.
///
/// # Errors
///
/// Returns an error if the command is empty, if `terminal` is set and no
/// emulator can be found, or if the process cannot be spawned.
pub fn launch(exec: &str, terminal: bool) -> Result<(), std::io::Error> {
    if exec.trim().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty Exec",
        ));
    }

    let argv = if terminal {
        let emulator = std::env::var("TERMINAL")
            .ok()
            .and_then(|t| find_in_path(&t))
            .or_else(|| TERMINAL_CANDIDATES.iter().find_map(|t| find_in_path(t)))
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no terminal emulator found; set $TERMINAL",
                )
            })?;
        terminal_argv(&emulator.to_string_lossy(), exec)
    } else {
        vec!["sh".to_string(), "-c".to_string(), exec.to_string()]
    };

    let (program, rest) = argv
        .split_first()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty argv"))?;

    std::process::Command::new(program)
        .args(rest)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    Ok(())
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Parse `contents` as if it were the file at `path`, which is what
    /// decides the desktop ID.
    ///
    /// `locales` is passed to the parser as well as to `convert`: it is a
    /// *filter*, so a locale left out here has its `Name[xx]` dropped before
    /// conversion ever sees it.
    fn parse_for(path: &str, contents: &str, locales: &[String]) -> fde::DesktopEntry {
        fde::DesktopEntry::from_str(PathBuf::from(path), contents, Some(locales))
            .unwrap_or_else(|e| panic!("{path} should parse: {e:?}"))
    }

    fn parse(path: &str, contents: &str) -> fde::DesktopEntry {
        parse_for(path, contents, &[String::from("en")])
    }

    fn convert_default(path: &str, contents: &str) -> Option<DesktopEntry> {
        convert(&parse(path, contents), &["en".to_string()], None)
    }

    #[test]
    fn a_plain_application_entry_is_listed() {
        let entry = convert_default(
            "/usr/share/applications/test.desktop",
            "[Desktop Entry]\nType=Application\nName=Test App\nExec=test-app %u\nComment=A test\nKeywords=testing;demo;\n",
        )
        .unwrap_or_else(|| panic!("should be listed"));

        assert_eq!(entry.id, "test");
        assert_eq!(entry.name, "Test App");
        assert_eq!(entry.exec, "test-app");
        assert_eq!(entry.comment.as_deref(), Some("A test"));
        assert!(entry.search_text.contains("test app"));
        assert!(entry.search_text.contains("testing"));
        assert!(entry.search_text.contains("demo"));
    }

    #[test]
    fn a_nested_directory_becomes_a_dashed_desktop_id() {
        // The old parser keyed on the bare filename, so `kde/foo.desktop` and
        // `foo.desktop` collided and one silently replaced the other.
        let entry = convert_default(
            "/usr/share/applications/kde/settings.desktop",
            "[Desktop Entry]\nType=Application\nName=Settings\nExec=settings\n",
        )
        .unwrap_or_else(|| panic!("should be listed"));
        assert_eq!(entry.id, "kde-settings");
    }

    #[test]
    fn hidden_and_nodisplay_entries_are_skipped() {
        assert!(convert_default(
            "/a/applications/h.desktop",
            "[Desktop Entry]\nType=Application\nName=H\nExec=h\nNoDisplay=true\n",
        )
        .is_none());
        assert!(convert_default(
            "/a/applications/h.desktop",
            "[Desktop Entry]\nType=Application\nName=H\nExec=h\nHidden=true\n",
        )
        .is_none());
    }

    #[test]
    fn non_application_entries_are_skipped() {
        assert!(convert_default(
            "/a/applications/link.desktop",
            "[Desktop Entry]\nType=Link\nName=A Link\nURL=https://example.com\n",
        )
        .is_none());
    }

    #[test]
    fn an_entry_whose_tryexec_is_missing_is_skipped() {
        // The spec's "is it installed" probe: listing it offers a launch that
        // cannot work.
        assert!(convert_default(
            "/a/applications/ghost.desktop",
            "[Desktop Entry]\nType=Application\nName=Ghost\nExec=ghost\nTryExec=/nonexistent/ghost\n",
        )
        .is_none());
        assert!(convert_default(
            "/a/applications/real.desktop",
            "[Desktop Entry]\nType=Application\nName=Real\nExec=real\nTryExec=/bin/sh\n",
        )
        .is_some());
    }

    #[test]
    fn only_show_in_and_not_show_in_follow_the_current_desktop() {
        let gnome_only = "[Desktop Entry]\nType=Application\nName=G\nExec=g\nOnlyShowIn=GNOME;\n";
        let not_hyprland =
            "[Desktop Entry]\nType=Application\nName=N\nExec=n\nNotShowIn=Hyprland;\n";
        let hypr = [String::from("hyprland")];

        assert!(convert(
            &parse("/a/applications/g.desktop", gnome_only),
            &[],
            Some(&hypr)
        )
        .is_none());
        assert!(convert(
            &parse("/a/applications/n.desktop", not_hyprland),
            &[],
            Some(&hypr)
        )
        .is_none());
        assert!(convert(
            &parse("/a/applications/g.desktop", gnome_only),
            &[],
            Some(&[String::from("gnome")])
        )
        .is_some());
    }

    #[test]
    fn without_a_current_desktop_show_in_keys_are_ignored() {
        // A systemd user unit can start before XDG_CURRENT_DESKTOP is set;
        // hiding every OnlyShowIn entry then would shrink the list invisibly.
        assert!(convert_default(
            "/a/applications/g.desktop",
            "[Desktop Entry]\nType=Application\nName=G\nExec=g\nOnlyShowIn=GNOME;\n",
        )
        .is_some());
    }

    #[test]
    fn a_localized_name_is_preferred_over_the_default() {
        let locales = [String::from("fr_FR")];
        let entry = convert(
            &parse_for(
                "/a/applications/l.desktop",
                "[Desktop Entry]\nType=Application\nName=Files\nName[fr]=Fichiers\nExec=files\n",
                &locales,
            ),
            &locales,
            None,
        )
        .unwrap_or_else(|| panic!("should be listed"));
        assert_eq!(entry.name, "Fichiers");
    }

    #[test]
    fn the_terminal_flag_is_read() {
        let tui = convert_default(
            "/a/applications/htop.desktop",
            "[Desktop Entry]\nType=Application\nName=htop\nExec=htop\nTerminal=true\n",
        )
        .unwrap_or_else(|| panic!("should be listed"));
        assert!(tui.terminal, "Terminal=true must be recorded");

        // Absent key defaults to false rather than being treated as a TUI.
        let gui = convert_default(
            "/a/applications/gui.desktop",
            "[Desktop Entry]\nType=Application\nName=Gui\nExec=gui\n",
        )
        .unwrap_or_else(|| panic!("should be listed"));
        assert!(!gui.terminal);
    }

    #[test]
    fn an_entry_without_an_exec_line_is_not_listed() {
        // Nothing to launch; showing it offers a row that does nothing.
        assert!(convert_default(
            "/a/applications/noexec.desktop",
            "[Desktop Entry]\nType=Application\nName=No Exec\n",
        )
        .is_none());
    }

    /// Write `name` (a path below the directory) with `contents`.
    fn write_entry(dir: &Path, name: &str, contents: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    fn app(name: &str) -> String {
        format!("[Desktop Entry]\nType=Application\nName={name}\nExec={name}\n")
    }

    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("obayebar_discover_{tag}"));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        /// A directory named `applications`, which is what the desktop ID is
        /// derived relative to.
        fn applications(&self, tag: &str) -> PathBuf {
            let dir = self.0.join(tag).join("applications");
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn the_highest_priority_directory_wins_a_duplicate_id() {
        // The old code kept the *last* match, so the system copy silently
        // replaced a user's ~/.local/share override.
        let scratch = ScratchDir::new("precedence");
        let user = scratch.applications("user");
        let system = scratch.applications("system");
        write_entry(&user, "editor.desktop", &app("Mine"));
        write_entry(&system, "editor.desktop", &app("Theirs"));

        let index = discover_in(vec![user, system]);
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries.first().map(|e| e.name.as_str()), Some("Mine"));
    }

    #[test]
    fn discovery_walks_subdirectories_and_ignores_other_files() {
        let scratch = ScratchDir::new("walk");
        let apps = scratch.applications("system");
        write_entry(&apps, "top.desktop", &app("Top"));
        write_entry(&apps, "kde/nested.desktop", &app("Nested"));
        write_entry(&apps, "notes.txt", "not an entry");
        write_entry(&apps, "mimeinfo.cache", "[MIME Cache]\n");

        let index = discover_in(vec![apps]);
        let ids: Vec<&str> = index.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["kde-nested", "top"]);
    }

    #[test]
    fn discovered_entries_are_sorted_by_name() {
        let scratch = ScratchDir::new("sorted");
        let apps = scratch.applications("system");
        for name in ["Zebra", "apple", "Mango"] {
            write_entry(&apps, &format!("{name}.desktop"), &app(name));
        }

        let index = discover_in(vec![apps]);
        let names: Vec<&str> = index.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["apple", "Mango", "Zebra"]);
    }

    #[test]
    fn an_entry_with_an_absolute_icon_path_resolves_to_that_file() {
        let scratch = ScratchDir::new("icon");
        let apps = scratch.applications("system");
        let icon = scratch.0.join("logo.png");
        std::fs::write(&icon, b"not really a png").unwrap();
        write_entry(
            &apps,
            "art.desktop",
            &format!(
                "[Desktop Entry]\nType=Application\nName=Art\nExec=art\nIcon={}\n",
                icon.display()
            ),
        );

        let index = discover_in(vec![apps]);
        assert_eq!(index.icon_paths.get("art"), Some(&icon));
    }

    #[test]
    fn application_dirs_have_no_duplicates() {
        // NIXOS_APPLICATIONS is usually also in XDG_DATA_DIRS, and scanning a
        // directory twice would make every entry in it lose to itself.
        let dirs = application_dirs();
        let unique: HashSet<&PathBuf> = dirs.iter().collect();
        assert_eq!(dirs.len(), unique.len(), "{dirs:?}");
    }

    #[test]
    fn an_absolute_icon_path_is_used_as_is() {
        assert_eq!(
            resolve_icon_path("/bin/sh", "hicolor"),
            Some(PathBuf::from("/bin/sh"))
        );
        assert_eq!(resolve_icon_path("/nonexistent/icon.png", "hicolor"), None);
    }

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
    fn sanitize_exec_strips_non_standard_field_codes() {
        // The old exact-match list missed anything not in it, so vendor and
        // wrapper entries kept tokens the program then saw as arguments.
        assert_eq!(sanitize_exec("app %w %X %somethingnew"), "app");
    }

    #[test]
    fn sanitize_exec_unescapes_a_literal_percent() {
        // %% is the spec's escape for a percent sign, not a field code.
        assert_eq!(sanitize_exec("app %% --rate"), "app % --rate");
    }

    #[test]
    fn sanitize_exec_strips_flatpak_forwarding_markers() {
        // Flatpak brackets a forwarded argument list we never supply; keeping
        // the markers made flatpak see them as positional arguments.
        assert_eq!(
            sanitize_exec(
                "/usr/bin/flatpak run --branch=stable --file-forwarding org.x.App @@u %U @@"
            ),
            "/usr/bin/flatpak run --branch=stable --file-forwarding org.x.App"
        );
    }

    #[test]
    fn sanitize_exec_keeps_quoting_for_the_shell() {
        // launch() runs this through sh, so the quotes must survive here. This
        // is also why the crate's `parse_exec` is not used: it splits on
        // whitespace and would break this line into five arguments.
        assert_eq!(
            sanitize_exec(r#"/bin/sh -c "exec /opt/app/bin/app --profile=default" %u"#),
            r#"/bin/sh -c "exec /opt/app/bin/app --profile=default""#
        );
    }

    #[test]
    fn terminal_argv_uses_dash_e_by_default() {
        assert_eq!(
            terminal_argv("/usr/bin/alacritty", "htop"),
            vec!["/usr/bin/alacritty", "-e", "sh", "-c", "htop"]
        );
    }

    #[test]
    fn terminal_argv_passes_bare_command_to_foot_and_kitty() {
        // Neither takes -e; the command is a trailing positional.
        assert_eq!(
            terminal_argv("foot", "htop"),
            vec!["foot", "sh", "-c", "htop"]
        );
        assert_eq!(
            terminal_argv("/nix/store/abc/bin/kitty", "ranger"),
            vec!["/nix/store/abc/bin/kitty", "sh", "-c", "ranger"]
        );
    }

    #[test]
    fn terminal_argv_uses_wezterm_start_subcommand() {
        assert_eq!(
            terminal_argv("wezterm", "btop"),
            vec!["wezterm", "start", "--", "sh", "-c", "btop"]
        );
    }

    #[test]
    fn launch_invalid_command() {
        assert!(launch("", false).is_err());
        assert!(launch("   ", false).is_err());
    }
}
