use crate::style;
use crate::Message;
use iced::widget::{button, container, text};
use iced::{Alignment, Element, Length};

pub fn view<'a>() -> Element<'a, Message> {
    let icon = text(style::ICON_POWER)
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_NORMAL)
        .color(style::M3_ERROR)
        .align_x(Alignment::Center);

    let btn = button(
        container(icon)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    )
    .on_press(Message::PowerClick)
    .style(style::transparent_button)
    .padding(style::PADDING_SMALL);

    container(btn)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .into()
}
