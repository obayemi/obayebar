use crate::services::gitlab::AuthState;
use crate::Message;
use iced::widget::{column, container, mouse_area, text, Column};
use iced::{Alignment, Element, Length};
use obayebar::style;

/// Render the bar entry: GitLab icon, plus a count badge when there are open
/// todos. Clicking opens the popup. The icon color also signals the auth state
/// (error tint when the token is missing or rejected).
pub fn view<'a>(auth: AuthState, count: usize, monitor: Option<String>) -> Element<'a, Message> {
    let icon_color = match auth {
        AuthState::Authenticated => style::M3_TERTIARY,
        AuthState::Missing => style::M3_ON_SURFACE_VARIANT,
        AuthState::Invalid => style::M3_ERROR,
    };

    let icon = text(style::ICON_TASK_ALT)
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_LARGE)
        .color(icon_color)
        .align_x(Alignment::Center);

    let mut stack: Column<'_, Message> = column![icon].spacing(2.0).align_x(Alignment::Center);

    if matches!(auth, AuthState::Authenticated) && count > 0 {
        let label = if count > 99 {
            "99+".to_string()
        } else {
            count.to_string()
        };
        stack = stack.push(
            text(label)
                .size(style::FONT_SIZE_SMALL)
                .color(icon_color)
                .align_x(Alignment::Center),
        );
    }

    let open_msg = Message::GitlabPanelOpen(monitor);
    let clickable = mouse_area(stack)
        .on_press(open_msg.clone())
        .on_enter(open_msg);

    container(clickable)
        .padding(style::PADDING_NORMAL)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .style(style::pill_container)
        .into()
}
