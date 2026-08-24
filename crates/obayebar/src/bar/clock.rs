use crate::Message;
use chrono::{DateTime, Local, Timelike};
use iced::widget::{column, container, text, Space};
use iced::{Alignment, Background, Element, Length};
use obayebar::style;

pub fn view(time: &DateTime<Local>) -> Element<'static, Message> {
    let day_abbr = time.format("%a").to_string();
    let day_num = time.format("%-d").to_string();
    let hour = format!("{:02}", time.hour());
    let minute = format!("{:02}", time.minute());

    let separator = container(Space::new().width(Length::Fill).height(1.0))
        .width(Length::FillPortion(4))
        .style(|_theme| container::Style {
            background: Some(Background::Color(style::with_alpha(
                style::M3_TERTIARY,
                0.2,
            ))),
            ..container::Style::default()
        });

    let clock_col = column![
        text(style::ICON_CALENDAR)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_TERTIARY)
            .align_x(Alignment::Center),
        text(format!("{day_abbr}\n{day_num}"))
            .size(style::FONT_SIZE_SMALLER)
            .color(style::M3_TERTIARY)
            .align_x(Alignment::Center),
        separator,
        text(format!("{hour}\n{minute}"))
            .size(style::FONT_SIZE_SMALLER)
            .font(iced::Font::MONOSPACE)
            .color(style::M3_TERTIARY)
            .align_x(Alignment::Center),
    ]
    .spacing(style::SPACING_SMALL)
    .align_x(Alignment::Center);

    container(clock_col)
        .padding(style::PADDING_NORMAL)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .style(style::pill_container)
        .into()
}
