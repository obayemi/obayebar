//! Keeping the application list current without polling.
//!
//! The bar is long-lived, so the list it holds has to react to installs and
//! removals rather than being rebuilt at every open (which is what the old
//! standalone launcher did — a full scan per keypress, and a stale list for the
//! first frame regardless). One inotify watch per application directory turns
//! that into: scan once at startup, then scan again only when something
//! actually changed.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::{Stream, StreamExt as _};
use inotify::{Inotify, WatchDescriptor, WatchMask};
use tokio_stream::wrappers::UnboundedReceiverStream;

use super::desktop_entry::{self, Index};

/// How long to keep swallowing events after the first one before rescanning.
///
/// A package install, and especially a `nixos-rebuild`, touches many entries in
/// a burst; without this each one would queue its own scan.
const DEBOUNCE: Duration = Duration::from_millis(400);

/// Events worth a rescan. `MODIFY` is deliberately absent: `CLOSE_WRITE`
/// already reports a finished write, and `MODIFY` fires once per `write()`
/// call on top of it.
const MASK: WatchMask = WatchMask::CREATE
    .union(WatchMask::DELETE)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::CLOSE_WRITE)
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF);

/// One directory to watch, and optionally the single name within it that
/// matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub dir: PathBuf,
    /// `None` watches everything in `dir` — an application directory itself.
    /// `Some(name)` watches one entry, used for the symlinks on the path to an
    /// application directory (see [`watch_targets`]).
    pub name: Option<OsString>,
}

/// The application list, rescanned whenever the filesystem says it changed.
///
/// Yields once immediately with the startup scan, so a caller does not need a
/// separate initial-load path.
pub fn index_stream() -> impl Stream<Item = Index> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        loop {
            let scanned = tokio::task::spawn_blocking(desktop_entry::discover).await;
            let Ok(index) = scanned else {
                log::error!("launcher: discovery panicked, giving up on rescans");
                break;
            };
            if tx.send(index).is_err() {
                break;
            }
            if !wait_for_change().await {
                // No watcher: the list stays as scanned. Worth saying out loud
                // once, since the symptom (a newly installed app never showing
                // up) is otherwise a mystery.
                log::warn!("launcher: not watching for changes; restart to pick up new apps");
                break;
            }
        }
    });

    UnboundedReceiverStream::new(rx)
}

/// Block until something under the watched directories changes.
///
/// Returns `false` when no watch could be established at all, which is the
/// caller's signal to stop rescanning rather than spin.
async fn wait_for_change() -> bool {
    let targets = watch_targets(&desktop_entry::application_dirs());
    let inotify = match Inotify::init() {
        Ok(inotify) => inotify,
        Err(err) => {
            log::warn!("launcher: cannot initialise inotify ({err})");
            return false;
        }
    };

    let mut watched: HashMap<WatchDescriptor, Option<OsString>> = HashMap::new();
    let watches = inotify.watches();
    for target in &targets {
        let mut watches = watches.clone();
        match watches.add(&target.dir, MASK) {
            // A directory can legitimately appear under two targets (an
            // application directory and the parent of a symlink to another);
            // inotify returns the same descriptor, and the broader watch wins.
            Ok(wd) => match watched.entry(wd) {
                std::collections::hash_map::Entry::Occupied(mut slot) => {
                    if target.name.is_none() {
                        slot.insert(None);
                    }
                }
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(target.name.clone());
                }
            },
            Err(err) => log::debug!("launcher: not watching {} ({err})", target.dir.display()),
        }
    }
    if watched.is_empty() {
        return false;
    }

    let mut events = match inotify.into_event_stream([0_u8; 4096]) {
        Ok(events) => events,
        Err(err) => {
            log::warn!("launcher: cannot read inotify events ({err})");
            return false;
        }
    };

    // Wait for the first event this actually cares about.
    loop {
        match events.next().await {
            Some(Ok(event)) => {
                if is_relevant(&watched, &event.wd, event.name.as_deref()) {
                    break;
                }
            }
            Some(Err(err)) => {
                log::warn!("launcher: inotify error ({err})");
                return false;
            }
            None => return false,
        }
    }

    // Then let the burst finish before telling the caller to rescan.
    loop {
        tokio::select! {
            () = tokio::time::sleep(DEBOUNCE) => break,
            event = events.next() => {
                if event.is_none() {
                    break;
                }
            }
        }
    }

    log::debug!("launcher: application directories changed, rescanning");
    true
}

/// Whether an event on `wd` for `name` is one we asked for.
fn is_relevant(
    watched: &HashMap<WatchDescriptor, Option<OsString>>,
    wd: &WatchDescriptor,
    name: Option<&std::ffi::OsStr>,
) -> bool {
    match watched.get(wd) {
        // A whole application directory: anything in it counts.
        Some(None) => true,
        // A symlink's parent: only that symlink counts, otherwise every file
        // created in a busy directory like /run would trigger a rescan.
        Some(Some(wanted)) => name.is_some_and(|name| name == wanted.as_os_str()),
        None => false,
    }
}

/// Directories to watch so that `dirs` staying current is actually observable.
///
/// Each application directory is watched directly, which covers ordinary
/// installs. What that misses is the shape every NixOS update takes: the
/// entries do not change, a *symlink* on the path to them is replaced
/// (`/run/current-system`, `~/.nix-profile`), and inotify resolved that symlink
/// when the watch was added — so the watch stays pointed at the old store path
/// and never fires. Watching the parent of each symlink on the path, filtered
/// to the symlink's own name, is what catches that swap.
#[must_use]
pub fn watch_targets(dirs: &[PathBuf]) -> Vec<Target> {
    let mut targets: Vec<Target> = Vec::new();
    let mut push = |target: Target| {
        if !targets.contains(&target) {
            targets.push(target);
        }
    };

    for dir in dirs {
        if dir.is_dir() {
            push(Target {
                dir: dir.clone(),
                name: None,
            });
        }
        for link in symlinks_on_path(dir) {
            let (Some(parent), Some(name)) = (link.parent(), link.file_name()) else {
                continue;
            };
            push(Target {
                dir: parent.to_path_buf(),
                name: Some(name.to_os_string()),
            });
        }
    }

    targets
}

/// Every component of `path` that is itself a symlink, outermost first.
fn symlinks_on_path(path: &Path) -> Vec<PathBuf> {
    let mut links = Vec::new();
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component);
        if std::fs::symlink_metadata(&prefix).is_ok_and(|m| m.is_symlink()) {
            links.push(prefix.clone());
        }
    }
    links
}

#[cfg(test)]
#[allow(clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("obayebar_watch_{tag}"));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn an_ordinary_directory_is_watched_whole() {
        let scratch = ScratchDir::new("plain");
        let apps = scratch.0.join("applications");
        std::fs::create_dir_all(&apps).unwrap();

        assert_eq!(
            watch_targets(&[apps.clone()]),
            vec![Target {
                dir: apps,
                name: None
            }]
        );
    }

    #[test]
    fn a_directory_that_does_not_exist_is_not_watched() {
        let scratch = ScratchDir::new("absent");
        assert!(watch_targets(&[scratch.0.join("nothing/applications")]).is_empty());
    }

    #[test]
    fn a_symlink_on_the_path_adds_a_watch_on_its_parent() {
        // The `/run/current-system` shape: the entries never change, the
        // symlink pointing at them is replaced. Watching only the resolved
        // directory would never see a rebuild.
        let scratch = ScratchDir::new("symlink");
        let generation = scratch.0.join("generation-1/share/applications");
        std::fs::create_dir_all(&generation).unwrap();
        let link = scratch.0.join("current");
        std::os::unix::fs::symlink(scratch.0.join("generation-1"), &link).unwrap();

        let apps = link.join("share/applications");
        let targets = watch_targets(&[apps.clone()]);

        assert!(
            targets.contains(&Target {
                dir: apps,
                name: None
            }),
            "{targets:?}"
        );
        assert!(
            targets.contains(&Target {
                dir: scratch.0.clone(),
                name: Some(OsString::from("current")),
            }),
            "{targets:?}"
        );
    }

    #[test]
    fn duplicate_directories_are_watched_once() {
        let scratch = ScratchDir::new("dupes");
        let apps = scratch.0.join("applications");
        std::fs::create_dir_all(&apps).unwrap();
        assert_eq!(watch_targets(&[apps.clone(), apps]).len(), 1);
    }

    #[test]
    fn only_the_named_entry_matters_for_a_symlink_watch() {
        let scratch = ScratchDir::new("relevance");
        let apps = scratch.0.join("applications");
        std::fs::create_dir_all(&apps).unwrap();

        let inotify = Inotify::init().unwrap();
        let wd = inotify.watches().add(&apps, MASK).unwrap();
        let mut watched = HashMap::new();
        watched.insert(wd.clone(), Some(OsString::from("current-system")));

        assert!(is_relevant(
            &watched,
            &wd,
            Some(std::ffi::OsStr::new("current-system"))
        ));
        // A busy directory like /run sees plenty of unrelated churn.
        assert!(!is_relevant(
            &watched,
            &wd,
            Some(std::ffi::OsStr::new("something-else"))
        ));

        // A whole-directory watch takes anything, including events with no
        // name at all (the directory itself being moved or deleted).
        watched.insert(wd.clone(), None);
        assert!(is_relevant(&watched, &wd, None));
    }
}
