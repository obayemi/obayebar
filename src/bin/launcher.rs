use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use obayebar::launcher::desktop_entry::DesktopEntry;
use obayebar::launcher::{self, desktop_entry, Launcher};
use obayebar::style;

struct LauncherInit {
    entries: Vec<DesktopEntry>,
    icon_paths: HashMap<String, PathBuf>,
    launch_counts: HashMap<String, u32>,
}

static INIT: OnceLock<LauncherInit> = OnceLock::new();

fn main() {
    env_logger::init();

    let icon_fonts = style::load_icon_font();

    // Load cache for instant startup; background refresh will update entries
    let cache = desktop_entry::load_cache();
    let launch_counts = desktop_entry::load_launch_counts();

    let (entries, icon_paths) = if cache.entries.is_empty() {
        // First launch: discover synchronously so UI isn't empty
        log::info!("No launcher cache, discovering entries...");
        let entries = desktop_entry::discover_entries();
        let icon_paths = desktop_entry::resolve_all_icon_paths(&entries);
        desktop_entry::save_cache(&desktop_entry::LauncherCache {
            entries: entries.clone(),
            icon_paths: icon_paths.clone(),
        });
        log::info!("Discovered {} desktop entries", entries.len());
        (entries, icon_paths)
    } else {
        log::info!("Loaded {} entries from cache", cache.entries.len());
        (cache.entries, cache.icon_paths)
    };

    INIT.get_or_init(|| LauncherInit {
        entries,
        icon_paths,
        launch_counts,
    });

    let result = iced_layershell::application(
        || {
            let init = INIT.get();
            let (entries, icon_paths, launch_counts) = init.map_or_else(
                || (Vec::new(), HashMap::new(), HashMap::new()),
                |i| {
                    (
                        i.entries.clone(),
                        i.icon_paths.clone(),
                        i.launch_counts.clone(),
                    )
                },
            );
            Launcher::new(entries, icon_paths, launch_counts)
        },
        Launcher::namespace,
        Launcher::update,
        Launcher::view,
    )
    .style(launcher::theme)
    .subscription(Launcher::subscription)
    .settings(Settings {
        layer_settings: LayerShellSettings {
            anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
            layer: Layer::Overlay,
            exclusive_zone: -1,
            size: Some((600, 500)),
            keyboard_interactivity: KeyboardInteractivity::Exclusive,
            ..LayerShellSettings::default()
        },
        fonts: icon_fonts,
        ..Settings::default()
    })
    .theme(launcher::theme_fn)
    .run();

    if let Err(err) = result {
        log::error!("obayebar-launcher exiting: {err}");
        std::process::exit(1);
    }
}
