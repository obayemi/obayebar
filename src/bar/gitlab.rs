use crate::services::gitlab::{AuthState, GitlabInfo};
use crate::Message;
use iced::widget::{column, container, mouse_area, text};
use iced::{Alignment, Element, Length};
use obayebar::style;

/// Render the bar entry: GitLab icon, plus a count badge when there are open
/// todos. Clicking opens the popup. The icon color also signals the auth state
/// (error tint when the token is missing or rejected).
pub fn view<'a>(info: &GitlabInfo, monitor: Option<&str>) -> Element<'a, Message> {
    let count = info.total;
    let (icon_color, badge_color) = match info.auth {
        AuthState::Authenticated => (style::M3_TERTIARY, style::M3_TERTIARY),
        AuthState::Missing => (style::M3_ON_SURFACE_VARIANT, style::M3_ON_SURFACE_VARIANT),
        AuthState::Invalid => (style::M3_ERROR, style::M3_ERROR),
    };

    let icon = text(style::ICON_TASK_ALT)
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_LARGE)
        .color(icon_color)
        .align_x(Alignment::Center);

    let badge: Element<'a, Message> = if matches!(info.auth, AuthState::Authenticated) && count > 0
    {
        let label = if count > 99 {
            "99+".to_string()
        } else {
            count.to_string()
        };
        text(label)
            .size(style::FONT_SIZE_SMALL)
            .color(badge_color)
            .align_x(Alignment::Center)
            .into()
    } else {
        text("").size(0.0).into()
    };

    let stack = column![icon, badge].spacing(2.0).align_x(Alignment::Center);

    let clickable = mouse_area(stack)
        .on_press(Message::GitlabPanelOpen(monitor.map(String::from)))
        .on_enter(Message::GitlabPanelOpen(monitor.map(String::from)));

    container(clickable)
        .padding(style::PADDING_NORMAL)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .style(style::pill_container)
        .into()
}
