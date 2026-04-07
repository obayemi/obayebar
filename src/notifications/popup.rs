use super::NotificationData;
use super::Urgency;
use crate::style;
use crate::Message;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Alignment, Border, Element, Length};

fn notification_card(notif: &NotificationData) -> Element<'_, Message> {
    let container_style = if notif.urgency == Urgency::Critical {
        style::notification_critical_container as fn(&iced::Theme) -> container::Style
    } else {
        style::notification_container
    };

    let icon_color = match notif.urgency {
        Urgency::Critical => style::M3_ON_ERROR,
        Urgency::Low => style::M3_ON_SURFACE,
        Urgency::Normal => style::M3_ON_SECONDARY_CONTAINER,
    };

    let icon_bg = match notif.urgency {
        Urgency::Critical => style::M3_ERROR,
        Urgency::Low => style::M3_SURFACE_CONTAINER_HIGHEST,
        Urgency::Normal => style::M3_SECONDARY_CONTAINER,
    };

    let badge = container(
        text(style::ICON_NOTIFICATIONS)
            .font(style::ICON_FONT)
            .size(16.0)
            .color(icon_color)
            .align_x(Alignment::Center),
    )
    .width(36.0)
    .height(36.0)
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(move |_theme| container::Style {
        background: Some(iced::Background::Color(icon_bg)),
        border: Border {
            radius: style::ROUNDING_FULL.into(),
            ..Border::default()
        },
        ..container::Style::default()
    });

    let summary = text(&notif.summary).size(13.0).color(style::M3_ON_SURFACE);

    let time_str = notif.time.format("%H:%M").to_string();
    let time_text = text(time_str)
        .size(10.0)
        .color(style::M3_ON_SURFACE_VARIANT);

    let header = row![
        summary,
        Space::new().width(Length::Fill).height(Length::Shrink),
        time_text
    ]
    .spacing(style::SPACING_SMALL)
    .align_y(Alignment::Center);

    let body_preview = text(&notif.body)
        .size(11.0)
        .color(style::M3_ON_SURFACE_VARIANT);

    let content = column![header, body_preview]
        .spacing(2.0)
        .width(Length::Fill);

    let card_content = row![badge, content]
        .spacing(style::SPACING_SMALLER)
        .align_y(Alignment::Start);

    let notif_id = notif.id;
    let dismiss_btn = button(
        text(style::ICON_CLOSE)
            .font(style::ICON_FONT)
            .size(14.0)
            .color(style::M3_ON_SURFACE),
    )
    .on_press(Message::NotifDismiss(notif_id))
    .style(style::transparent_button)
    .padding(2.0);

    let full_row = row![card_content, dismiss_btn]
        .spacing(4.0)
        .width(Length::Fill);

    container(full_row)
        .padding(style::PADDING_NORMAL)
        .width(Length::Fill)
        .style(container_style)
        .into()
}

pub fn view(popups: &[NotificationData]) -> Element<'_, Message> {
    let mut content = column![]
        .spacing(style::SPACING_SMALLER)
        .width(Length::Fill);

    for notif in popups {
        content = content.push(notification_card(notif));
    }

    container(content)
        .padding(style::PADDING_LARGE)
        .width(Length::Fill)
        .height(Length::Shrink)
        .into()
}
