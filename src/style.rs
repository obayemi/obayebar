#![allow(dead_code)]

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
pub const ICON_WIFI_4: &str = "\u{F065}";
pub const ICON_WIFI_3: &str = "\u{EBE1}";
pub const ICON_WIFI_2: &str = "\u{EBD6}";
pub const ICON_WIFI_1: &str = "\u{EBE4}";
pub const ICON_WIFI_0: &str = "\u{F0B0}";
pub const ICON_WIFI_OFF: &str = "\u{E648}";
pub const ICON_CABLE: &str = "\u{EFE6}";
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
pub const ICON_LANGUAGE: &str = "\u{E894}";
pub const ICON_DESKTOP: &str = "\u{E30C}";
pub const ICON_NOTIFICATIONS_NONE: &str = "\u{E7F5}";

/// Load the Material Symbols font from the system or `OBAYEBAR_FONT_DIR` env var.
pub fn load_icon_font() -> Vec<Cow<'static, [u8]>> {
    let font_paths = [
        // Environment variable set by nix flake
        std::env::var("OBAYEBAR_FONT_DIR")
            .map(|dir| format!("{dir}/MaterialSymbolsOutlined.ttf"))
            .ok(),
        // Common system paths
        Some("/run/current-system/sw/share/fonts/TTF/MaterialSymbolsOutlined.ttf".into()),
        Some(format!(
            "{}/.local/share/fonts/MaterialSymbolsOutlined.ttf",
            std::env::var("HOME").unwrap_or_default()
        )),
    ];

    for path in font_paths.into_iter().flatten() {
        if let Ok(data) = std::fs::read(&path) {
            log::info!("Loaded icon font from {path}");
            return vec![Cow::Owned(data)];
        }
    }

    // Try fontconfig as last resort
    if let Ok(output) = std::process::Command::new("fc-match")
        .args(["Material Symbols Outlined", "-f", "%{file}"])
        .output()
    {
        let path = String::from_utf8_lossy(&output.stdout);
        let path = path.trim();
        if path.contains("MaterialSymbols") {
            if let Ok(data) = std::fs::read(path) {
                log::info!("Loaded icon font via fontconfig: {path}");
                return vec![Cow::Owned(data)];
            }
        }
    }

    log::warn!("Material Symbols Outlined font not found - icons will not render");
    Vec::new()
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
            radius: ROUNDING_NORMAL.into(),
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
            radius: ROUNDING_NORMAL.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// Notification center sidebar container
pub fn notif_center_container(theme: &iced::Theme) -> container::Style {
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
