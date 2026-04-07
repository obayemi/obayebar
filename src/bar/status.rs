use crate::services::audio::AudioInfo;
use crate::services::battery::BatteryInfo;
use crate::services::network::NetworkInfo;
use crate::style;
use crate::Message;
use iced::widget::{column, container, text};
use iced::{Alignment, Element, Length};

fn status_icon(icon_name: &str, color: iced::Color) -> Element<'_, Message> {
    text(icon_name)
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_LARGE)
        .color(color)
        .align_x(Alignment::Center)
        .into()
}

pub fn view<'a>(
    battery: &BatteryInfo,
    network: &NetworkInfo,
    audio: &AudioInfo,
) -> Element<'a, Message> {
    let mut icons = column![]
        .spacing(style::SPACING_SMALLER / 2.0)
        .align_x(Alignment::Center);

    icons = icons.push(status_icon(audio.icon_name, style::M3_SECONDARY));
    icons = icons.push(status_icon(network.icon_name, style::M3_SECONDARY));

    if battery.present {
        let battery_color = if battery.percentage <= 20.0 {
            style::M3_ERROR
        } else {
            style::M3_SECONDARY
        };
        icons = icons.push(status_icon(battery.icon_name, battery_color));
    }

    container(icons)
        .padding(style::PADDING_NORMAL)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .style(style::pill_container)
        .into()
}
