use crate::services::audio::AudioInfo;
use crate::services::battery::BatteryInfo;
use crate::services::bluetooth::BluetoothInfo;
use crate::services::network::NetworkInfo;
use crate::services::sysinfo::SysInfo;
use crate::style;
use crate::Message;
use iced::widget::{column, container, mouse_area, text};
use iced::{Alignment, Color, Element, Length};

/// Threshold above which usage is considered elevated.
const ELEVATED_THRESHOLD: f32 = 70.0;
/// Threshold above which usage is considered critical.
const CRITICAL_THRESHOLD: f32 = 90.0;

fn usage_color(percent: f32) -> iced::Color {
    if percent >= CRITICAL_THRESHOLD {
        style::M3_ERROR
    } else if percent >= ELEVATED_THRESHOLD {
        style::M3_TERTIARY
    } else {
        style::M3_SECONDARY
    }
}

/// Approximate line height matching iced cosmic-text rendering.
const ICON_LINE_HEIGHT: f32 = style::FONT_SIZE_LARGE * 1.3;

/// Render a single icon at the standard bar size.
fn single_icon(icon: &str, color: Color) -> Element<'_, Message> {
    text(icon)
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_LARGE)
        .color(color)
        .align_x(Alignment::Center)
        .into()
}

/// Render two icons split diagonally within the same space as one icon.
/// Shows the top-right half of `icon1` and the bottom-left half of `icon2`.
fn split_icon<'a>(
    icon1: &'a str,
    color1: Color,
    icon2: &'a str,
    color2: Color,
) -> Element<'a, Message> {
    let half_h = ICON_LINE_HEIGHT / 2.0;

    // Top half: icon1 clipped to upper portion, aligned right
    let top_half = container(
        text(icon1)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(color1),
    )
    .width(Length::Fill)
    .height(Length::Fixed(half_h))
    .align_x(Alignment::End)
    .clip(true);

    // Bottom half: icon2 inside a full-height inner container, outer clips to
    // half height aligned to bottom so only the lower portion is visible
    let inner = container(
        text(icon2)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(color2),
    )
    .width(Length::Fill)
    .height(Length::Fixed(ICON_LINE_HEIGHT))
    .align_x(Alignment::Start);

    let bottom_half = container(inner)
        .width(Length::Fill)
        .height(Length::Fixed(half_h))
        .align_y(Alignment::End)
        .clip(true);

    column![top_half, bottom_half]
        .spacing(0)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .into()
}

/// Find the worst elevated metric to display in the bar icon.
fn sysinfo_icon_view(sysinfo: &SysInfo) -> Element<'_, Message> {
    let mut elevated: Vec<(f32, &str)> = Vec::new();
    if sysinfo.cpu_percent >= ELEVATED_THRESHOLD {
        elevated.push((sysinfo.cpu_percent, style::ICON_SPEED));
    }
    if sysinfo.gpu_percent >= ELEVATED_THRESHOLD {
        elevated.push((sysinfo.gpu_percent, style::ICON_GPU));
    }
    if sysinfo.ram_percent >= ELEVATED_THRESHOLD {
        elevated.push((sysinfo.ram_percent, style::ICON_MEMORY));
    }

    // Sort by severity (highest first)
    elevated.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    if let Some(&(pct1, icon1)) = elevated.first() {
        if let Some(&(pct2, icon2)) = elevated.get(1) {
            split_icon(icon1, usage_color(pct1), icon2, usage_color(pct2))
        } else {
            single_icon(icon1, usage_color(pct1))
        }
    } else {
        single_icon(style::ICON_CHECK_CIRCLE, style::M3_SECONDARY)
    }
}

pub fn view<'a>(
    battery: &BatteryInfo,
    network: &NetworkInfo,
    audio: &AudioInfo,
    bluetooth: &BluetoothInfo,
    sysinfo: &'a SysInfo,
    monitor: Option<&str>,
) -> Element<'a, Message> {
    let mut icons = column![]
        .spacing(style::SPACING_SMALLER / 2.0)
        .align_x(Alignment::Center);

    let audio_icon = mouse_area(
        text(audio.icon_name)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_SECONDARY)
            .align_x(Alignment::Center),
    )
    .on_enter(Message::AudioPanelOpen(monitor.map(String::from)))
    .on_press(Message::AudioOpenPavucontrol);

    let network_icon = mouse_area(
        text(network.icon_name)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_SECONDARY)
            .align_x(Alignment::Center),
    )
    .on_enter(Message::NetworkPanelOpen(monitor.map(String::from)));

    let bluetooth_icon = mouse_area(
        text(bluetooth.icon_name)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_SECONDARY)
            .align_x(Alignment::Center),
    )
    .on_enter(Message::BluetoothPanelOpen(monitor.map(String::from)));

    let sysinfo_icon = mouse_area(sysinfo_icon_view(sysinfo))
        .on_enter(Message::SysinfoPanelOpen(monitor.map(String::from)));

    icons = icons.push(audio_icon);
    icons = icons.push(bluetooth_icon);
    icons = icons.push(network_icon);
    icons = icons.push(sysinfo_icon);

    if battery.present {
        let battery_color = if battery.percentage <= 20.0 {
            style::M3_ERROR
        } else {
            style::M3_SECONDARY
        };
        let battery_icon = mouse_area(
            text(battery.icon_name)
                .font(style::ICON_FONT)
                .size(style::FONT_SIZE_LARGE)
                .color(battery_color)
                .align_x(Alignment::Center),
        )
        .on_enter(Message::BatteryPanelOpen(monitor.map(String::from)));
        icons = icons.push(battery_icon);
    }

    container(icons)
        .padding(style::PADDING_NORMAL)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .style(style::pill_container)
        .into()
}
