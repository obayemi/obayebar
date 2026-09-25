use super::rotated_text::{render_rotated_text, truncate_with_ellipsis};
use crate::services::hyprland::WindowInfo;
use crate::Message;
use ab_glyph::FontArc;
use iced::widget::{container, image};
use iced::{Alignment, Element, Length};
use obayebar::style;

pub fn view(window: Option<&WindowInfo>, font: Option<&FontArc>) -> Element<'static, Message> {
    let title = window.map_or_else(
        || "Desktop".into(),
        |w| {
            let parts: Vec<&str> = w.title.split(&['\u{2013}', '\u{2014}', '-'][..]).collect();
            let raw = parts
                .last()
                .map_or_else(|| w.title.clone(), |s| s.trim().to_string());
            truncate_with_ellipsis(&raw, 20)
        },
    );

    let content: Element<'_, Message> = if let Some(f) = font {
        let handle = render_rotated_text(f, &title, style::FONT_SIZE_LARGE, style::M3_PRIMARY);
        image(handle).content_fit(iced::ContentFit::None).into()
    } else {
        iced::widget::text(title)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_PRIMARY)
            .align_x(Alignment::Center)
            .into()
    };

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}
