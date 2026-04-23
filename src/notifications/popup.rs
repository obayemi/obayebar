use crate::services::notifications::{NotificationData, Urgency};
use crate::Message;
use iced::widget::{column, container, image, mouse_area, row, text, Space};
use iced::{Alignment, Border, Element, Length};
use obayebar::style;

/// Icon strip width matches the two-line card height so it renders as a square.
/// summary (13*1.3) + spacing (2) + body (11*1.3) + padding (10*2) ≈ 53
const ICON_STRIP_SIZE: f32 = 53.0;

fn notification_card(notif: &NotificationData, hovered: bool) -> Element<'_, Message> {
    let container_style = match (notif.urgency == Urgency::Critical, hovered) {
        (true, true) => {
            style::notification_critical_container_hovered as fn(&iced::Theme) -> container::Style
        }
        (true, false) => style::notification_critical_container,
        (false, true) => style::notification_container_hovered,
        (false, false) => style::notification_container,
    };

    let icon_strip = build_icon_strip(notif);
    let notif_id = notif.id;

    let card = container(
        row![icon_strip, build_right_content(notif)]
            .width(Length::Fill)
            .height(Length::Shrink),
    )
    .width(Length::Fill)
    .style(container_style);

    mouse_area(card)
        .on_press(Message::NotifActivate(notif_id))
        .on_right_press(Message::NotifDismiss(notif_id))
        .on_enter(Message::NotifHoverEnter(notif_id))
        .on_exit(Message::NotifHoverExit(notif_id))
        .into()
}

fn build_icon_strip(notif: &NotificationData) -> Element<'_, Message> {
    let icon_color = match notif.urgency {
        Urgency::Critical => style::M3_ON_ERROR,
        Urgency::Low => style::M3_ON_SURFACE_VARIANT,
        Urgency::Normal => style::M3_ON_SECONDARY_CONTAINER,
    };

    let icon_bg = match notif.urgency {
        Urgency::Critical => style::M3_ERROR,
        Urgency::Low => style::M3_SURFACE_CONTAINER_HIGHEST,
        Urgency::Normal => style::M3_SECONDARY_CONTAINER,
    };

    let left_rounded = iced::border::Radius {
        top_left: style::ROUNDING_EXTRA_SMALL,
        top_right: 0.0,
        bottom_right: 0.0,
        bottom_left: style::ROUNDING_EXTRA_SMALL,
    };

    notif.image.as_ref().map_or_else(
        || {
            container(
                text(style::ICON_NOTIFICATIONS)
                    .font(style::ICON_FONT)
                    .size(style::FONT_SIZE_LARGE)
                    .color(icon_color)
                    .align_x(Alignment::Center),
            )
            .width(ICON_STRIP_SIZE)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(move |_theme| container::Style {
                background: Some(iced::Background::Color(icon_bg)),
                border: Border {
                    radius: left_rounded,
                    ..Border::default()
                },
                ..container::Style::default()
            })
            .into()
        },
        |img| {
            let handle = image::Handle::from_rgba(img.width, img.height, img.rgba.clone());
            container(
                image(handle)
                    .width(ICON_STRIP_SIZE)
                    .height(Length::Fill)
                    .content_fit(iced::ContentFit::Cover),
            )
            .width(ICON_STRIP_SIZE)
            .height(Length::Fill)
            .clip(true)
            .style(move |_theme| container::Style {
                border: Border {
                    radius: left_rounded,
                    ..Border::default()
                },
                ..container::Style::default()
            })
            .into()
        },
    )
}

fn build_right_content(notif: &NotificationData) -> Element<'_, Message> {
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

    let text_content = column![header, body_preview]
        .spacing(2.0)
        .width(Length::Fill);

    row![text_content]
        .spacing(4.0)
        .padding([style::PADDING_NORMAL, style::PADDING_NORMAL])
        .align_y(Alignment::Start)
        .width(Length::Fill)
        .into()
}

fn overflow_card(count: usize) -> Element<'static, Message> {
    let label = if count == 1 {
        "1 other notification".to_string()
    } else {
        format!("{count} other notifications")
    };
    container(
        text(label)
            .size(style::FONT_SIZE_SMALLER)
            .color(style::M3_ON_SURFACE_VARIANT)
            .align_x(Alignment::Center),
    )
    .padding([style::PADDING_NORMAL, style::PADDING_NORMAL])
    .width(Length::Fill)
    .align_x(Alignment::Center)
    .style(style::notification_container)
    .into()
}

pub fn view(
    popups: &[NotificationData],
    hovered_id: Option<u32>,
    visible: usize,
    overflow: usize,
) -> Element<'_, Message> {
    let mut content = column![]
        .spacing(style::SPACING_SMALLER)
        .width(Length::Fill);

    for notif in popups.iter().take(visible) {
        let hovered = hovered_id == Some(notif.id);
        content = content.push(notification_card(notif, hovered));
    }

    if overflow > 0 {
        content = content.push(overflow_card(overflow));
    }

    container(content)
        .padding(style::PADDING_LARGE)
        .width(Length::Fill)
        .height(Length::Shrink)
        .into()
}
