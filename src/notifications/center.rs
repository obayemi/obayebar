use super::NotificationData;
use super::Urgency;
use crate::style;
use crate::Message;
use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Border, Element, Length};

fn notification_entry(notif: &NotificationData) -> Element<'_, Message> {
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

    let app_name = text(&notif.app_name)
        .size(10.0)
        .color(style::M3_ON_SURFACE_VARIANT);

    let time_str = notif.time.format("%H:%M").to_string();
    let time_label = text(time_str)
        .size(10.0)
        .color(style::M3_ON_SURFACE_VARIANT);

    let dot = text(" \u{2022} ")
        .size(10.0)
        .color(style::M3_ON_SURFACE_VARIANT);

    let info_row = row![app_name, dot, time_label].align_y(Alignment::Center);

    let summary = text(&notif.summary).size(13.0).color(style::M3_ON_SURFACE);

    let body = text(&notif.body)
        .size(11.0)
        .color(style::M3_ON_SURFACE_VARIANT);

    let notif_id = notif.id;
    let close_btn = button(
        container(text("Close").size(11.0).color(style::M3_ON_SURFACE_VARIANT))
            .padding([style::PADDING_SMALL, style::PADDING_NORMAL]),
    )
    .on_press(Message::NotifDismiss(notif_id))
    .style(|_theme, _status| button::Style {
        background: Some(iced::Background::Color(style::M3_SURFACE_CONTAINER_HIGH)),
        text_color: style::M3_ON_SURFACE_VARIANT,
        border: Border {
            radius: style::ROUNDING_FULL.into(),
            ..Border::default()
        },
        shadow: iced::Shadow::default(),
        snap: false,
    })
    .padding(0.0);

    let actions_row = row![close_btn].spacing(style::SPACING_SMALLER);

    let content = column![info_row, summary, body, actions_row]
        .spacing(4.0)
        .width(Length::Fill);

    let card = row![badge, content]
        .spacing(style::SPACING_SMALLER)
        .align_y(Alignment::Start);

    container(card)
        .padding(style::PADDING_NORMAL)
        .width(Length::Fill)
        .style(container_style)
        .into()
}

pub fn view(notifications: &[NotificationData]) -> Element<'_, Message> {
    let header = row![
        text("Notifications").size(16.0).color(style::M3_ON_SURFACE),
        Space::new().width(Length::Fill).height(Length::Shrink),
        button(
            text("Clear all")
                .size(11.0)
                .color(style::M3_ON_SURFACE_VARIANT),
        )
        .on_press(Message::NotifClearAll)
        .style(style::transparent_button)
        .padding(4.0),
    ]
    .align_y(Alignment::Center)
    .padding([0.0, style::PADDING_NORMAL]);

    let mut list = column![]
        .spacing(style::SPACING_SMALLER)
        .width(Length::Fill);

    if notifications.is_empty() {
        list = list.push(
            container(
                column![
                    text(style::ICON_NOTIFICATIONS_NONE)
                        .font(style::ICON_FONT)
                        .size(48.0)
                        .color(style::M3_ON_SURFACE_VARIANT)
                        .align_x(Alignment::Center),
                    text("No notifications")
                        .size(13.0)
                        .color(style::M3_ON_SURFACE_VARIANT)
                        .align_x(Alignment::Center),
                ]
                .spacing(style::SPACING_NORMAL)
                .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .padding(style::PADDING_LARGE * 2.0)
            .align_x(Alignment::Center),
        );
    } else {
        for notif in notifications {
            list = list.push(notification_entry(notif));
        }
    }

    let content = column![header, scrollable(list).height(Length::Fill)]
        .spacing(style::SPACING_NORMAL)
        .width(Length::Fill)
        .height(Length::Fill);

    container(content)
        .padding(style::PADDING_LARGE)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(style::notif_center_container)
        .into()
}
