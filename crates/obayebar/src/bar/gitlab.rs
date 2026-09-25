use super::widgets::{icon_text, panel_trigger};
use crate::panel::PanelKind;
use crate::services::gitlab::AuthState;
use crate::Message;
use iced::widget::{column, text};
use iced::{Alignment, Element};
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

    let badge = (matches!(auth, AuthState::Authenticated) && count > 0).then(|| {
        let label = if count > 99 {
            "99+".to_string()
        } else {
            count.to_string()
        };
        text(label)
            .size(style::FONT_SIZE_SMALL)
            .color(icon_color)
            .align_x(Alignment::Center)
    });

    let stack = column![
        icon_text(style::ICON_TASK_ALT, style::FONT_SIZE_LARGE, icon_color),
        badge,
    ]
    .spacing(2.0)
    .align_x(Alignment::Center);

    panel_trigger(PanelKind::Gitlab, monitor, stack)
}
