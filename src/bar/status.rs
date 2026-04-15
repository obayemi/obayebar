use crate::services::audio::AudioInfo;
use crate::services::battery::BatteryInfo;
use crate::services::bluetooth::BluetoothInfo;
use crate::services::network::NetworkInfo;
use crate::services::sysinfo::SysInfo;
use crate::style;
use crate::Message;
use iced::widget::{column, container, mouse_area, text};
use iced::{Alignment, Element, Length};

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

fn sysinfo_view(sysinfo: &SysInfo) -> Element<'_, Message> {
    let cpu_high = sysinfo.cpu_percent >= ELEVATED_THRESHOLD;
    let ram_high = sysinfo.ram_percent >= ELEVATED_THRESHOLD;

    match (cpu_high, ram_high) {
        // Both elevated: show both icons with diagonal layout
        (true, true) => {
            let cpu_icon = container(
                text(style::ICON_SPEED)
                    .font(style::ICON_FONT)
                    .size(style::FONT_SIZE_LARGER)
                    .color(usage_color(sysinfo.cpu_percent)),
            )
            .width(Length::Fill)
            .align_x(Alignment::End);

            let ram_icon = container(
                text(style::ICON_MEMORY)
                    .font(style::ICON_FONT)
                    .size(style::FONT_SIZE_LARGER)
                    .color(usage_color(sysinfo.ram_percent)),
            )
            .width(Length::Fill)
            .align_x(Alignment::Start);

            column![cpu_icon, ram_icon].spacing(0).into()
        }
        // Only CPU elevated
        (true, false) => text(style::ICON_SPEED)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(usage_color(sysinfo.cpu_percent))
            .align_x(Alignment::Center)
            .into(),
        // Only RAM elevated
        (false, true) => text(style::ICON_MEMORY)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(usage_color(sysinfo.ram_percent))
            .align_x(Alignment::Center)
            .into(),
        // Everything fine
        (false, false) => text(style::ICON_CHECK_CIRCLE)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_SECONDARY)
            .align_x(Alignment::Center)
            .into(),
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

    icons = icons.push(sysinfo_view(sysinfo));

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

    icons = icons.push(audio_icon);
    icons = icons.push(bluetooth_icon);
    icons = icons.push(network_icon);

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
