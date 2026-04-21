use std::sync::OnceLock;

use iced_layershell::reexport::{Anchor, KeyboardInteractivity, Layer};
use iced_layershell::settings::{LayerShellSettings, Settings};
use obayebar::launcher::{self, desktop_entry::DesktopEntry, Launcher};
use obayebar::style;

static ENTRIES: OnceLock<Vec<DesktopEntry>> = OnceLock::new();

fn main() {
    env_logger::init();

    let icon_fonts = style::load_icon_font();
    let entries = launcher::desktop_entry::discover_entries();
    log::info!("Discovered {} desktop entries", entries.len());
    ENTRIES.get_or_init(|| entries);

    let result = iced_layershell::application(
        || Launcher::new(ENTRIES.get().cloned().unwrap_or_default()),
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
