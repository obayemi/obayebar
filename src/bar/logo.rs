use crate::Message;
use iced::widget::{container, text};
use iced::{Alignment, Element, Length};
use obayebar::style;

pub fn view() -> Element<'static, Message> {
    let icon = text(style::ICON_DEPLOYED_CODE)
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_LARGE * 1.2)
        .color(style::M3_TERTIARY)
        .align_x(Alignment::Center);

    container(icon)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .into()
}
