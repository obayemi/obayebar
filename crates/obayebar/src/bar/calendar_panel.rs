use super::widgets::{icon_button, panel_body, scroll_lines, separator};
use crate::panel::PanelKind;
use crate::Message;
use chrono::{DateTime, Datelike, Local, NaiveDate, WeekdaySet};
use iced::widget::{column, container, mouse_area, row, text, Row};
use iced::{Alignment, Background, Color, Element, Length};
use obayebar::calendar::{Day, DayKind, Month, Pager, Paging, Step, Week, WEEK_START};
use obayebar::style;

pub fn view(now: &DateTime<Local>, pager: Pager) -> Element<'static, Message> {
    let today = now.date_naive();
    let month = pager.month(today);

    let content = column![
        today_header(now),
        separator(),
        navigation(month),
        month_grid(month, today),
    ]
    .spacing(style::SPACING_NORMAL)
    .width(Length::Fill);

    panel_body(PanelKind::Calendar, content)
}

/// The current time and the long date, centered above the grid.
fn today_header(now: &DateTime<Local>) -> Element<'static, Message> {
    column![
        text(now.format("%H:%M").to_string())
            .size(style::CALENDAR_TIME_SIZE)
            .font(iced::Font::MONOSPACE)
            .color(style::M3_ON_SURFACE),
        text(now.format("%A %-d %B %Y").to_string())
            .size(style::FONT_SIZE_SMALLER)
            .color(style::M3_ON_SURFACE_VARIANT),
    ]
    .width(Length::Fill)
    .align_x(Alignment::Center)
    .into()
}

/// The weekday header and the month's week rows, paged by scrolling.
fn month_grid(month: Month, today: NaiveDate) -> Element<'static, Message> {
    mouse_area(
        column(
            std::iter::once(weekday_header())
                .chain(month.weeks().into_iter().map(|week| week_row(&week, today))),
        )
        .width(Length::Fill),
    )
    .on_scroll(|delta| Message::Calendar(Paging::Scroll(scroll_lines(delta))))
    .into()
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
    let labels = WeekdaySet::ALL
        .iter(WEEK_START)
        .map(|day| cell(day.to_string(), style::M3_ON_SURFACE_VARIANT, None));
    grid_row(cell(String::new(), Color::TRANSPARENT, None), labels)
}

fn week_row(week: &Week, today: NaiveDate) -> Element<'static, Message> {
    let number = cell(week.number.to_string(), style::M3_TERTIARY, None);
    grid_row(number, week.days.iter().map(|&day| day_cell(day, today)))
}

fn day_cell(day: Day, today: NaiveDate) -> Element<'static, Message> {
    let (color, fill) = day_style(day.kind(today));
    cell(day.date.day().to_string(), color, fill)
}

/// Text colour and fill for a grid cell, decided by what the day represents:
/// today stands out filled solid, an in-month day reads at full strength,
/// and a day spilling from a neighbouring month fades.
const fn day_style(kind: DayKind) -> (Color, Option<Color>) {
    match kind {
        DayKind::Today => (style::M3_ON_PRIMARY, Some(style::M3_PRIMARY)),
        DayKind::InMonth => (style::M3_ON_SURFACE, None),
        DayKind::Spill => (style::with_alpha(style::M3_ON_SURFACE_VARIANT, 0.4), None),
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

#[cfg(test)]
mod tests {
    use super::day_style;
    use obayebar::calendar::DayKind;
    use obayebar::style;

    #[test]
    fn today_is_filled_solid() {
        assert_eq!(
            day_style(DayKind::Today),
            (style::M3_ON_PRIMARY, Some(style::M3_PRIMARY))
        );
    }

    #[test]
    fn an_in_month_day_reads_at_full_strength() {
        assert_eq!(day_style(DayKind::InMonth), (style::M3_ON_SURFACE, None));
    }

    #[test]
    fn a_spill_day_fades() {
        assert_eq!(
            day_style(DayKind::Spill),
            (style::with_alpha(style::M3_ON_SURFACE_VARIANT, 0.4), None)
        );
    }
}
