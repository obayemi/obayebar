#![allow(dead_code)]
// Style functions are typically passed as arguments to iced widget builders;
// #[must_use] adds no value and clutters every function signature.
#![allow(clippy::must_use_candidate)]

use iced::widget::container;
use iced::{Background, Border, Color, Font};
use std::borrow::Cow;

// Material Design 3 dark theme baseline palette
pub const M3_PRIMARY: Color = Color::from_rgb(0.816, 0.737, 1.0);
pub const M3_ON_PRIMARY: Color = Color::from_rgb(0.220, 0.118, 0.447);
pub const M3_PRIMARY_CONTAINER: Color = Color::from_rgb(0.310, 0.216, 0.545);
pub const M3_ON_PRIMARY_CONTAINER: Color = Color::from_rgb(0.918, 0.875, 1.0);

pub const M3_SECONDARY: Color = Color::from_rgb(0.800, 0.761, 0.863);
pub const M3_ON_SECONDARY: Color = Color::from_rgb(0.200, 0.176, 0.255);
pub const M3_SECONDARY_CONTAINER: Color = Color::from_rgb(0.290, 0.267, 0.345);
pub const M3_ON_SECONDARY_CONTAINER: Color = Color::from_rgb(0.914, 0.882, 0.973);

pub const M3_TERTIARY: Color = Color::from_rgb(0.937, 0.722, 0.784);
pub const M3_ON_TERTIARY: Color = Color::from_rgb(0.286, 0.145, 0.196);
pub const M3_TERTIARY_CONTAINER: Color = Color::from_rgb(0.408, 0.271, 0.333);
pub const M3_ON_TERTIARY_CONTAINER: Color = Color::from_rgb(1.0, 0.851, 0.894);

pub const M3_ERROR: Color = Color::from_rgb(0.949, 0.722, 0.710);
pub const M3_ON_ERROR: Color = Color::from_rgb(0.376, 0.078, 0.063);
pub const M3_ERROR_CONTAINER: Color = Color::from_rgb(0.549, 0.114, 0.094);
pub const M3_ON_ERROR_CONTAINER: Color = Color::from_rgb(0.976, 0.871, 0.859);

pub const M3_SURFACE: Color = Color::from_rgb(0.122, 0.114, 0.129);
pub const M3_ON_SURFACE: Color = Color::from_rgb(0.906, 0.882, 0.898);
pub const M3_ON_SURFACE_VARIANT: Color = Color::from_rgb(0.792, 0.769, 0.816);
pub const M3_SURFACE_CONTAINER: Color = Color::from_rgb(0.161, 0.149, 0.169);
pub const M3_SURFACE_CONTAINER_LOW: Color = Color::from_rgb(0.137, 0.125, 0.145);
pub const M3_SURFACE_CONTAINER_HIGH: Color = Color::from_rgb(0.192, 0.176, 0.200);
pub const M3_SURFACE_CONTAINER_HIGHEST: Color = Color::from_rgb(0.224, 0.208, 0.231);

pub const M3_OUTLINE: Color = Color::from_rgb(0.576, 0.561, 0.600);
pub const M3_OUTLINE_VARIANT: Color = Color::from_rgb(0.286, 0.271, 0.310);

// Layout constants matching caelestia-shell defaults exactly
// From BarConfig: innerWidth = 40
pub const BAR_INNER_WIDTH: f32 = 40.0;
// From BarWrapper: padding = max(Appearance.padding.smaller, border.thickness) = 7
// contentWidth = innerWidth + padding * 2 = 40 + 14 = 54
pub const BAR_PADDING: f32 = 7.0;
pub const BAR_WIDTH: u32 = 54;
pub const NOTIF_WIDTH: u32 = 400;

// Spacing (from AppearanceConfig)
pub const SPACING_SMALL: f32 = 7.0;
pub const SPACING_SMALLER: f32 = 10.0;
pub const SPACING_NORMAL: f32 = 12.0;
pub const SPACING_LARGER: f32 = 15.0;
pub const SPACING_LARGE: f32 = 20.0;

// Padding (from AppearanceConfig)
pub const PADDING_SMALL: f32 = 5.0;
pub const PADDING_SMALLER: f32 = 7.0;
pub const PADDING_NORMAL: f32 = 10.0;
pub const PADDING_LARGER: f32 = 12.0;
pub const PADDING_LARGE: f32 = 15.0;

// Rounding (from AppearanceConfig)
pub const ROUNDING_EXTRA_SMALL: f32 = 5.0;
pub const ROUNDING_SMALL: f32 = 12.0;
pub const ROUNDING_NORMAL: f32 = 17.0;
pub const ROUNDING_LARGE: f32 = 25.0;
pub const ROUNDING_FULL: f32 = 1000.0;

// Font sizes (from AppearanceConfig)
pub const FONT_SIZE_SMALL: f32 = 11.0;
pub const FONT_SIZE_SMALLER: f32 = 12.0;
pub const FONT_SIZE_NORMAL: f32 = 13.0;
pub const FONT_SIZE_LARGER: f32 = 15.0;
pub const FONT_SIZE_LARGE: f32 = 18.0;
pub const FONT_SIZE_EXTRA_LARGE: f32 = 28.0;

// Material Symbols font
pub const ICON_FONT: Font = Font::with_name("Material Symbols Outlined");

// Material Symbols codepoints (ligatures don't work in cosmic-text)
pub const ICON_CALENDAR: &str = "\u{EBCC}";
pub const ICON_POWER: &str = "\u{F8C7}";
pub const ICON_VOLUME_UP: &str = "\u{E050}";
pub const ICON_VOLUME_DOWN: &str = "\u{E04D}";
pub const ICON_VOLUME_MUTE: &str = "\u{E04E}";
pub const ICON_VOLUME_OFF: &str = "\u{E04F}";
pub const ICON_BATTERY_FULL: &str = "\u{E1A5}";
pub const ICON_BATTERY_CHARGING_FULL: &str = "\u{E1A3}";
pub const ICON_BATTERY_0: &str = "\u{EBDC}";
pub const ICON_BATTERY_1: &str = "\u{F09C}";
pub const ICON_BATTERY_2: &str = "\u{F09D}";
pub const ICON_BATTERY_3: &str = "\u{F09E}";
pub const ICON_BATTERY_4: &str = "\u{F09F}";
pub const ICON_BATTERY_5: &str = "\u{F0A0}";
pub const ICON_BATTERY_6: &str = "\u{F0A1}";
pub const ICON_BATTERY_CHARGING_20: &str = "\u{F0A2}";
pub const ICON_BATTERY_CHARGING_30: &str = "\u{F0A3}";
pub const ICON_BATTERY_CHARGING_50: &str = "\u{F0A4}";
pub const ICON_BATTERY_CHARGING_60: &str = "\u{F0A5}";
pub const ICON_BATTERY_CHARGING_90: &str = "\u{F0A7}";
pub const ICON_BOLT: &str = "\u{EA0B}";
pub const ICON_ECO: &str = "\u{EA35}";
pub const ICON_WIFI_4: &str = "\u{F065}";
pub const ICON_WIFI_3: &str = "\u{EBE1}";
pub const ICON_WIFI_2: &str = "\u{EBD6}";
pub const ICON_WIFI_1: &str = "\u{EBE4}";
pub const ICON_WIFI_0: &str = "\u{F0B0}";
pub const ICON_WIFI_OFF: &str = "\u{E648}";
pub const ICON_CABLE: &str = "\u{EFE6}";
pub const ICON_VPN: &str = "\u{E32A}";
pub const ICON_DEPLOYED_CODE: &str = "\u{F720}";
pub const ICON_NOTIFICATIONS: &str = "\u{E7F5}";
pub const ICON_CLOSE: &str = "\u{E5CD}";
pub const ICON_EXPAND_LESS: &str = "\u{E5CE}";
pub const ICON_EXPAND_MORE: &str = "\u{E5CF}";
pub const ICON_SPEED: &str = "\u{E9E4}";
pub const ICON_MEMORY: &str = "\u{E322}";
pub const ICON_BLUETOOTH: &str = "\u{E1A7}";
pub const ICON_BLUETOOTH_CONNECTED: &str = "\u{E1A8}";
pub const ICON_BLUETOOTH_DISABLED: &str = "\u{E1A9}";
pub const ICON_BLUETOOTH_SEARCHING: &str = "\u{E1AA}";
pub const ICON_DELETE: &str = "\u{E872}";
pub const ICON_CHECK_CIRCLE: &str = "\u{E86C}";
pub const ICON_GPU: &str = "\u{E30D}";
pub const ICON_ARROW_UPWARD: &str = "\u{E5D8}";
pub const ICON_ARROW_DOWNWARD: &str = "\u{E5DB}";
pub const ICON_THERMOSTAT: &str = "\u{E1FF}";
pub const ICON_LANGUAGE: &str = "\u{E894}";
pub const ICON_DESKTOP: &str = "\u{E30C}";
pub const ICON_NOTIFICATIONS_NONE: &str = "\u{E7F5}";
pub const ICON_SETTINGS: &str = "\u{E8B8}";
pub const ICON_AUTORENEW: &str = "\u{E863}";

pub const AUDIO_PANEL_WIDTH: u32 = 320;
pub const NETWORK_PANEL_WIDTH: u32 = 300;
pub const BATTERY_PANEL_WIDTH: u32 = 200;
pub const BLUETOOTH_PANEL_WIDTH: u32 = 280;
pub const SYSINFO_PANEL_WIDTH: u32 = 280;
/// Visual gap between the bar and popup panels, rendered as transparent padding
/// inside the panel window so the `mouse_area` covers the gap.
pub const PANEL_GAP: f32 = 8.0;
pub const PANEL_GAP_PX: u32 = 8;

/// Line-height multiplier: iced cosmic-text renders text taller than the font size.
const LINE_HEIGHT: f32 = 1.3;

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::as_conversions
)]
pub fn audio_panel_height(sink_count: usize) -> u32 {
    let container_padding = PADDING_LARGE * 2.0;

    // Header row: icon + "Audio" text
    let header = FONT_SIZE_LARGE * LINE_HEIGHT;

    // Outer column: 4 items (header, sink_list, separator, volume_section) → 3 gaps
    let outer_spacing = SPACING_NORMAL * 3.0;

    // Sink list column (spacing = 2.0):
    //   "Output device" label + N sink entries (or 1 "no devices" fallback)
    let sink_label = FONT_SIZE_SMALLER * LINE_HEIGHT;
    let n = sink_count.max(1) as f32;
    // Each sink button: text + vertical padding
    let per_sink = PADDING_SMALL.mul_add(2.0, FONT_SIZE_NORMAL * LINE_HEIGHT);
    // N entries + label = (N+1) items → N gaps of 2px
    let sink_list = sink_label + n * per_sink + n * 2.0;

    // Separator
    let separator = 1.0;

    // Volume section column (spacing = SPACING_SMALL):
    //   label + slider row
    let vol_label = FONT_SIZE_SMALLER * LINE_HEIGHT;
    // Slider row: mute button (icon + padding) determines height
    let vol_row = PADDING_SMALL.mul_add(2.0, FONT_SIZE_LARGE * LINE_HEIGHT);
    let volume_section = vol_label + SPACING_SMALL + vol_row;

    // Extra margin to account for widget intrinsic sizing (slider track, button chrome, etc.)
    let safety = 30.0;

    (container_padding + header + outer_spacing + sink_list + separator + volume_section + safety)
        .ceil() as u32
}

/// Compute the popup notification window height for `notif_count` cards.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::as_conversions
)]
pub fn notif_popup_height(notif_count: usize) -> u32 {
    let container_padding = PADDING_LARGE * 2.0;
    let n = notif_count.max(1) as f32;

    // Each notification card: two text lines + padding, at least icon strip size (53px)
    let summary_line = FONT_SIZE_NORMAL * LINE_HEIGHT;
    let body_line = FONT_SIZE_SMALL * LINE_HEIGHT;
    let card_inner = summary_line.mul_add(1.0, 2.0 + body_line);
    let card_height = PADDING_NORMAL.mul_add(2.0, card_inner).max(53.0);

    // N cards with SPACING_SMALLER gaps between them
    let cards = (n - 1.0).max(0.0).mul_add(SPACING_SMALLER, n * card_height);

    let safety = 20.0;
    (container_padding + cards + safety).ceil() as u32
}

/// Compute the network panel window height for `ap_count` visible networks.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::as_conversions
)]
/// Compute the network panel height. Each entry in `conn_type_groups` is a per-type count.
pub fn network_panel_height(
    ap_count: usize,
    conn_type_groups: &[usize],
    wifi_enabled: bool,
) -> u32 {
    let container_padding = PADDING_LARGE * 2.0;

    // Header: icon + "Network" + toggle
    let header = FONT_SIZE_LARGE * LINE_HEIGHT;
    // Separator
    let separator = 1.0;

    let per_entry = PADDING_SMALL.mul_add(2.0, FONT_SIZE_NORMAL * LINE_HEIGHT);
    let label_height = FONT_SIZE_SMALLER * LINE_HEIGHT;

    // Active connections: each type group has a label + entries, groups separated by spacing
    let active_section = if conn_type_groups.is_empty() {
        0.0
    } else {
        let mut h = 0.0;
        for (i, &count) in conn_type_groups.iter().enumerate() {
            if i > 0 {
                h += SPACING_NORMAL;
            }
            let n = count as f32;
            h += label_height + (n - 1.0).max(0.0).mul_add(2.0, n * per_entry);
        }
        // separator + spacing after all groups
        h + separator + SPACING_NORMAL
    };

    if !wifi_enabled {
        // Header + separator + active connections + "Wi-Fi is off" text
        let off_text = FONT_SIZE_NORMAL * LINE_HEIGHT;
        let spacing = SPACING_NORMAL * 2.0;
        let safety = 20.0;
        return (container_padding
            + header
            + separator
            + spacing
            + active_section
            + off_text
            + safety)
            .ceil() as u32;
    }

    // 2 gaps between header, separator, network_list
    let outer_spacing = SPACING_NORMAL * 2.0;

    // "Wi-Fi networks" label + N entries
    let label = FONT_SIZE_SMALLER * LINE_HEIGHT;
    let n = ap_count.max(1) as f32;
    let network_list = label + (n - 1.0).max(0.0).mul_add(2.0, n * per_entry);

    let safety = 20.0;
    (container_padding
        + header
        + separator
        + outer_spacing
        + active_section
        + network_list
        + safety)
        .ceil() as u32
}

/// Compute the battery panel window height.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
pub fn battery_panel_height(has_power_profiles: bool) -> u32 {
    let container_padding = PADDING_LARGE * 2.0;
    let header = FONT_SIZE_LARGE * LINE_HEIGHT;
    let gauge = 140.0; // GAUGE_SIZE in battery_panel
    let time_label = FONT_SIZE_SMALLER * LINE_HEIGHT;
    // 2 gaps between header, gauge, time_label
    let outer_spacing = SPACING_NORMAL * 2.0;

    let profiles_section = if has_power_profiles {
        let separator = 1.0;
        let label = FONT_SIZE_SMALLER * LINE_HEIGHT;
        // Profile buttons: icon (FONT_SIZE_NORMAL) + label (FONT_SIZE_SMALL) + spacing + padding
        let button_height = PADDING_SMALL.mul_add(
            2.0,
            FONT_SIZE_SMALL.mul_add(LINE_HEIGHT, FONT_SIZE_NORMAL * LINE_HEIGHT) + 2.0,
        );
        // 3 extra SPACING_NORMAL gaps (separator, label, buttons row)
        SPACING_NORMAL.mul_add(3.0, separator + label + button_height)
    } else {
        0.0
    };

    let safety = 10.0;
    (container_padding + header + gauge + time_label + outer_spacing + profiles_section + safety)
        .ceil() as u32
}

/// Compute the bluetooth panel window height.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::as_conversions
)]
pub fn bluetooth_panel_height(
    paired_count: usize,
    nearby_count: usize,
    powered: bool,
    discovering: bool,
) -> u32 {
    let container_padding = PADDING_LARGE * 2.0;
    let header = FONT_SIZE_LARGE * LINE_HEIGHT;
    let separator = 1.0;

    if !powered {
        // Just header + separator + "Bluetooth is off" text
        let off_text = FONT_SIZE_NORMAL * LINE_HEIGHT;
        let spacing = SPACING_NORMAL * 2.0;
        let safety = 20.0;
        return (container_padding + header + separator + spacing + off_text + safety).ceil()
            as u32;
    }

    // Discovery toggle button row
    let discovery_btn = PADDING_SMALL.mul_add(2.0, FONT_SIZE_NORMAL * LINE_HEIGHT);

    // Each device entry height
    let per_entry = PADDING_SMALL.mul_add(
        2.0,
        FONT_SIZE_SMALL.mul_add(LINE_HEIGHT, FONT_SIZE_NORMAL * LINE_HEIGHT),
    );

    // Paired devices section
    let label = FONT_SIZE_SMALLER * LINE_HEIGHT;
    let n = paired_count.max(1) as f32;
    let device_list = label + (n - 1.0).max(0.0).mul_add(2.0, n * per_entry);

    // Nearby section (only when discovering with unpaired devices)
    let nearby_section = if discovering && nearby_count > 0 {
        let nearby_label = FONT_SIZE_SMALLER * LINE_HEIGHT;
        let m = nearby_count as f32;
        SPACING_NORMAL + nearby_label + (m - 1.0).max(0.0).mul_add(2.0, m * per_entry)
    } else {
        0.0
    };

    // 4 gaps: header→separator, separator→discovery, discovery→separator2, separator2→list
    let outer_spacing = SPACING_NORMAL * 4.0;
    let safety = 20.0;
    (container_padding
        + header
        + separator * 2.0
        + outer_spacing
        + discovery_btn
        + device_list
        + nearby_section
        + safety)
        .ceil() as u32
}

/// Compute the sysinfo panel window height (2x2 grid: CPU, GPU, RAM, Network).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::as_conversions
)]
pub fn sysinfo_panel_height() -> u32 {
    let container_padding = PADDING_LARGE * 2.0;
    let header = FONT_SIZE_LARGE * LINE_HEIGHT;
    // Each grid cell: 90px gauge + 2px gap + label + optional temp line
    let gauge_size = 90.0;
    let gauge_label = FONT_SIZE_SMALL * LINE_HEIGHT;
    let temp_line = FONT_SIZE_SMALL * LINE_HEIGHT;
    let per_row = gauge_size + 2.0 + gauge_label + temp_line;
    // 2 rows + 2 gaps (header→row1, row1→row2)
    let outer_spacing = SPACING_NORMAL * 2.0;
    let safety = 15.0;
    (container_padding + header + per_row * 2.0 + outer_spacing + safety).ceil() as u32
}

fn find_outlined_font(dir: &str) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.starts_with("MaterialSymbolsOutlined")
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ttf"))
        {
            return Some(path);
        }
    }
    None
}

/// Load the Material Symbols font from the system or `OBAYEBAR_FONT_DIR` env var.
pub fn load_icon_font() -> Vec<Cow<'static, [u8]>> {
    // Nixpkgs' material-symbols package now ships a variable font named
    // `MaterialSymbolsOutlined[FILL,GRAD,opsz,wght].ttf`; older releases
    // shipped `MaterialSymbolsOutlined.ttf`. Scan the directory for either.
    let font_dirs = [
        std::env::var("OBAYEBAR_FONT_DIR").ok(),
        Some("/run/current-system/sw/share/fonts/TTF".into()),
        Some(format!(
            "{}/.local/share/fonts",
            std::env::var("HOME").unwrap_or_default()
        )),
    ];

    for dir in font_dirs.into_iter().flatten() {
        if let Some(path) = find_outlined_font(&dir) {
            if let Ok(data) = std::fs::read(&path) {
                log::info!("Loaded icon font from {}", path.display());
                return vec![Cow::Owned(data)];
            }
        }
    }

    // Try fontdb as last resort
    {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let query = fontdb::Query {
            families: &[fontdb::Family::Name("Material Symbols Outlined")],
            ..fontdb::Query::default()
        };
        if let Some(id) = db.query(&query) {
            if let Some(face) = db.face(id) {
                if let fontdb::Source::File(ref path) = face.source {
                    if let Ok(data) = std::fs::read(path) {
                        log::info!("Loaded icon font via fontdb: {}", path.display());
                        return vec![Cow::Owned(data)];
                    }
                }
            }
        }
    }

    log::warn!("Material Symbols Outlined font not found - icons will not render");
    Vec::new()
}

/// Load the system sans-serif font for vector text rendering via `ab_glyph`.
pub fn load_vector_font() -> Option<ab_glyph::FontArc> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let query = fontdb::Query {
        families: &[fontdb::Family::SansSerif],
        ..fontdb::Query::default()
    };
    let id = db.query(&query)?;
    let face = db.face(id)?;
    if let fontdb::Source::File(ref path) = face.source {
        let data = std::fs::read(path).ok()?;
        let font = ab_glyph::FontArc::try_from_vec(data).ok()?;
        log::info!("Loaded vector font from {}", path.display());
        Some(font)
    } else {
        None
    }
}

/// Apply alpha to a color
pub const fn with_alpha(color: Color, alpha: f32) -> Color {
    Color { a: alpha, ..color }
}

/// Pill-shaped container with surface container background
pub fn pill_container(theme: &iced::Theme) -> container::Style {
    let _ = theme;
    container::Style {
        background: Some(Background::Color(with_alpha(M3_SURFACE_CONTAINER, 0.85))),
        border: Border {
            radius: ROUNDING_FULL.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// Notification card container
pub fn notification_container(theme: &iced::Theme) -> container::Style {
    let _ = theme;
    container::Style {
        background: Some(Background::Color(with_alpha(M3_SURFACE_CONTAINER, 0.95))),
        border: Border {
            radius: ROUNDING_EXTRA_SMALL.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// Notification card container (hovered)
pub fn notification_container_hovered(theme: &iced::Theme) -> container::Style {
    let _ = theme;
    container::Style {
        background: Some(Background::Color(with_alpha(
            M3_SURFACE_CONTAINER_HIGHEST,
            0.98,
        ))),
        border: Border {
            radius: ROUNDING_EXTRA_SMALL.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// Critical notification container
pub fn notification_critical_container(theme: &iced::Theme) -> container::Style {
    let _ = theme;
    container::Style {
        background: Some(Background::Color(with_alpha(M3_SECONDARY_CONTAINER, 0.95))),
        border: Border {
            radius: ROUNDING_EXTRA_SMALL.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// Critical notification container (hovered)
pub fn notification_critical_container_hovered(theme: &iced::Theme) -> container::Style {
    let _ = theme;
    container::Style {
        background: Some(Background::Color(with_alpha(M3_TERTIARY_CONTAINER, 0.98))),
        border: Border {
            radius: ROUNDING_EXTRA_SMALL.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// Outer wrapper for popup panels — near-invisible background ensures the
/// compositor includes the gap area in the input region.
pub fn panel_wrapper_container(theme: &iced::Theme) -> container::Style {
    let _ = theme;
    container::Style {
        background: Some(Background::Color(with_alpha(iced::Color::BLACK, 0.01))),
        ..container::Style::default()
    }
}

/// Audio panel overlay container
pub fn audio_panel_container(theme: &iced::Theme) -> container::Style {
    let _ = theme;
    container::Style {
        background: Some(Background::Color(with_alpha(
            M3_SURFACE_CONTAINER_LOW,
            0.92,
        ))),
        border: Border {
            radius: ROUNDING_NORMAL.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// Transparent button style (no background, no border)
pub fn transparent_button(
    _theme: &iced::Theme,
    _status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    iced::widget::button::Style {
        background: None,
        text_color: M3_ON_SURFACE,
        border: Border::default(),
        shadow: iced::Shadow::default(),
        snap: false,
    }
}
