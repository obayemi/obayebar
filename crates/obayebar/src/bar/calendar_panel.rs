use super::widgets::{icon_button, panel_with_exit, separator};
use crate::panel::PanelKind;
use crate::Message;
use chrono::{DateTime, Datelike, Local, NaiveDate};
use iced::widget::{column, container, mouse_area, row, text, Row};
use iced::{mouse, Alignment, Background, Color, Element, Length};
use obayebar::calendar::{Day, DayKind, Month, Pager, Paging, Step, Week, WEEKDAYS};
use obayebar::style;

pub fn view(now: &DateTime<Local>, pager: Pager) -> Element<'static, Message> {
    let today = now.date_naive();
    let month = pager.month(today);

    let clock = column![
        text(now.format("%H:%M").to_string())
            .size(style::CALENDAR_TIME_SIZE)
            .font(iced::Font::MONOSPACE)
            .color(style::M3_ON_SURFACE),
        text(now.format("%A %-d %B %Y").to_string())
            .size(style::FONT_SIZE_SMALLER)
            .color(style::M3_ON_SURFACE_VARIANT),
    ]
    .width(Length::Fill)
    .align_x(Alignment::Center);

    let grid = mouse_area(
        column(
            std::iter::once(weekday_header())
                .chain(month.weeks().into_iter().map(|week| week_row(&week, today))),
        )
        .width(Length::Fill),
    )
    .on_scroll(|delta| {
        Message::Calendar(Paging::Scroll(match delta {
            mouse::ScrollDelta::Lines { y, .. } => y,
            mouse::ScrollDelta::Pixels { y, .. } => y / 120.0,
        }))
    });

    let content = column![clock, separator(), navigation(month), grid]
        .spacing(style::SPACING_NORMAL)
        .width(Length::Fill);

    let panel = container(content)
        .padding(style::PADDING_LARGE)
        .width(Length::Fill)
        .height(Length::Shrink)
        .style(style::panel_container);

    panel_with_exit(PanelKind::Calendar, panel.into())
}

fn navigation(month: Month) -> Element<'static, Message> {
    row![
        icon_button(
            style::ICON_CHEVRON_LEFT,
            style::M3_ON_SURFACE_VARIANT,
            Message::Calendar(Paging::Page(Step::Previous)),
        ),
        text(month.first_day().format("%B %Y").to_string())
            .size(style::FONT_SIZE_NORMAL)
            .color(style::M3_ON_SURFACE)
            .width(Length::Fill)
            .align_x(Alignment::Center),
        icon_button(
            style::ICON_CHEVRON_RIGHT,
            style::M3_ON_SURFACE_VARIANT,
            Message::Calendar(Paging::Page(Step::Next)),
        ),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn weekday_header() -> Element<'static, Message> {
    let labels = WEEKDAYS
        .iter()
        .map(|day| cell(day.to_string(), style::M3_ON_SURFACE_VARIANT, None));
    grid_row(cell(String::new(), style::M3_TERTIARY, None), labels)
}

fn week_row(week: &Week, today: NaiveDate) -> Element<'static, Message> {
    let number = cell(week.number.to_string(), style::M3_TERTIARY, None);
    grid_row(number, week.days.iter().map(|&day| day_cell(day, today)))
}

fn day_cell(day: Day, today: NaiveDate) -> Element<'static, Message> {
    let label = day.date.day().to_string();
    match day.kind(today) {
        DayKind::Today => cell(label, style::M3_ON_PRIMARY, Some(style::M3_PRIMARY)),
        DayKind::InMonth => cell(label, style::M3_ON_SURFACE, None),
        DayKind::Spill => cell(
            label,
            style::with_alpha(style::M3_ON_SURFACE_VARIANT, 0.4),
            None,
        ),
    }
}

fn grid_row(
    leading: Element<'static, Message>,
    days: impl Iterator<Item = Element<'static, Message>>,
) -> Element<'static, Message> {
    Row::with_children(std::iter::once(leading).chain(days))
        .width(Length::Fill)
        .into()
}

fn cell(label: String, color: Color, fill: Option<Color>) -> Element<'static, Message> {
    let disc = container(text(label).size(style::FONT_SIZE_SMALLER).color(color))
        .center(style::CALENDAR_CELL)
        .style(move |_theme| container::Style {
            background: fill.map(Background::Color),
            border: iced::Border::default().rounded(style::ROUNDING_FULL),
            ..container::Style::default()
        });
    container(disc).center_x(Length::Fill).into()
}
