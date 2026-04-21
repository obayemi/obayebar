use std::sync::OnceLock;

use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use obayebar::launcher::desktop_entry::LauncherCache;
use obayebar::launcher::{self, desktop_entry, Launcher};
use obayebar::style;

static INIT: OnceLock<(LauncherCache, bool)> = OnceLock::new();

fn main() {
    env_logger::init();

    let icon_fonts = style::load_icon_font();

    // Load cache for instant startup; background refresh will update entries
    let cache = desktop_entry::load_cache();

    let (cache, fresh) = if cache.entries.is_empty() {
        // First launch: discover synchronously so UI isn't empty
        log::info!("No launcher cache, discovering entries...");
        let entries = desktop_entry::discover_entries();
        let icon_paths = desktop_entry::resolve_all_icon_paths(&entries);
        let cache = LauncherCache {
            entries,
            icon_paths,
            launch_counts: cache.launch_counts,
        };
        desktop_entry::save_cache(&cache);
        log::info!("Discovered {} desktop entries", cache.entries.len());
        (cache, true)
    } else {
        log::info!("Loaded {} entries from cache", cache.entries.len());
        (cache, false)
    };

    INIT.get_or_init(|| (cache, fresh));

    let result = iced_layershell::application(
        || {
            let (cache, fresh) = INIT
                .get()
                .map_or_else(|| (LauncherCache::default(), false), |v| (v.0.clone(), v.1));
            Launcher::new(cache.entries, cache.icon_paths, cache.launch_counts, fresh)
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
