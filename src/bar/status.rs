use crate::services::audio::AudioInfo;
use crate::services::battery::BatteryInfo;
use crate::services::network::NetworkInfo;
use crate::style;
use crate::Message;
use iced::widget::{column, container, mouse_area, text};
use iced::{Alignment, Element, Length};

pub fn view<'a>(
    battery: &BatteryInfo,
    network: &NetworkInfo,
    audio: &AudioInfo,
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

    icons = icons.push(audio_icon);
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
