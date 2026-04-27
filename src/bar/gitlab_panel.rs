use super::widgets::{hover_button_style, panel_with_exit, separator};
use crate::services::gitlab::{self, AuthState, GitlabInfo, TodoItem, TODO_PAGE_PATH};
use crate::Message;
use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Alignment, Background, Border, Color, Element, Length};
use obayebar::style;

/// Right-side scrollbar styled like the launcher's.
fn scrollable_style(_theme: &iced::Theme, status: scrollable::Status) -> scrollable::Style {
    let scroller_color = match status {
        scrollable::Status::Hovered {
            is_vertical_scrollbar_hovered: true,
            ..
        }
        | scrollable::Status::Dragged {
            is_vertical_scrollbar_dragged: true,
            ..
        } => style::M3_PRIMARY,
        scrollable::Status::Hovered { .. } => style::M3_ON_SURFACE_VARIANT,
        _ => style::M3_OUTLINE,
    };
    let rail = scrollable::Rail {
        background: Some(Background::Color(style::with_alpha(
            style::M3_SURFACE_CONTAINER,
            0.5,
        ))),
        border: Border {
            radius: 3.0.into(),
            ..Border::default()
        },
        scroller: scrollable::Scroller {
            background: Background::Color(scroller_color),
            border: Border {
                radius: 3.0.into(),
                ..Border::default()
            },
        },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: Background::Color(style::with_alpha(style::M3_SURFACE_CONTAINER, 0.9)),
            border: Border::default(),
            shadow: iced::Shadow::default(),
            icon: style::M3_ON_SURFACE,
        },
    }
}

fn todo_entry(item: &TodoItem) -> Element<'_, Message> {
    let action_label = format!(
        "{} {}",
        gitlab::format_action(&item.action),
        gitlab::format_target_type(&item.target_type),
    );
    let title = if item.title.is_empty() {
        "(no title)".to_string()
    } else {
        item.title.clone()
    };

    let header = row![
        text(action_label)
            .size(style::FONT_SIZE_SMALL)
            .color(style::M3_TERTIARY),
        Space::new().width(Length::Fill),
        text(item.project.clone())
            .size(style::FONT_SIZE_SMALL)
            .color(style::M3_ON_SURFACE_VARIANT),
    ]
    .spacing(style::SPACING_SMALL)
    .align_y(Alignment::Center);

    let body = text(title)
        .size(style::FONT_SIZE_NORMAL)
        .color(style::M3_ON_SURFACE);

    let content = column![header, body].spacing(2.0).width(Length::Fill);

    let entry = button(content)
        .on_press(Message::GitlabOpenUrl(item.url.clone()))
        .style(hover_button_style(Color::TRANSPARENT, style::M3_ON_SURFACE))
        .padding(style::PADDING_ENTRY)
        .width(Length::Fill);
    entry.into()
}

fn pill_action_button<'a>(icon: &'a str, label: &'a str, msg: Message) -> Element<'a, Message> {
    let row = row![
        text(icon)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_NORMAL)
            .color(style::M3_ON_SURFACE),
        text(label)
            .size(style::FONT_SIZE_NORMAL)
            .color(style::M3_ON_SURFACE),
    ]
    .spacing(style::SPACING_SMALL)
    .align_y(Alignment::Center);

    button(row)
        .on_press(msg)
        .style(hover_button_style(
            style::with_alpha(style::M3_SURFACE_CONTAINER_HIGH, 0.9),
            style::M3_ON_SURFACE,
        ))
        .padding(style::PADDING_ENTRY)
        .into()
}

fn auth_setup_view(info: &GitlabInfo) -> Element<'_, Message> {
    let title = if matches!(info.auth, AuthState::Invalid) {
        "GitLab token rejected"
    } else {
        "GitLab token not configured"
    };
    let intro = text(title)
        .size(style::FONT_SIZE_NORMAL)
        .color(style::M3_ON_SURFACE);

    let path_label = gitlab::token_file_path().map_or_else(
        || "$XDG_CONFIG_HOME/obayebar/gitlab_token".to_string(),
        |p| p.display().to_string(),
    );

    let instructions = text(format!(
        "1. Create a Personal Access Token with the read_api scope on GitLab and copy it.\n2. Click \"Paste token from clipboard\" below.\n\nAlternatively save it to {path_label} or export OBAYEBAR_GITLAB_TOKEN."
    ))
    .size(style::FONT_SIZE_SMALL)
    .color(style::M3_ON_SURFACE_VARIANT);

    let create_url = format!(
        "{}/-/user_settings/personal_access_tokens?name=obayebar&scopes=read_api",
        info.host,
    );

    let buttons = column![
        pill_action_button(
            style::ICON_KEY,
            "Create access token on gitlab",
            Message::GitlabOpenUrl(create_url),
        ),
        pill_action_button(
            style::ICON_CONTENT_PASTE,
            "Paste token from clipboard",
            Message::GitlabPasteToken,
        ),
        pill_action_button(
            style::ICON_FOLDER,
            "Open token file",
            Message::GitlabOpenTokenFile,
        ),
        pill_action_button(
            style::ICON_REFRESH,
            "Reload token",
            Message::GitlabReloadToken,
        ),
    ]
    .spacing(style::SPACING_SMALL)
    .width(Length::Fill);

    let mut col = column![intro, instructions, buttons]
        .spacing(style::SPACING_NORMAL)
        .width(Length::Fill);

    if let Some(err) = info.error.as_deref() {
        col = col.push(
            text(format!("Last error: {err}"))
                .size(style::FONT_SIZE_SMALL)
                .color(style::M3_ERROR),
        );
    }

    col.into()
}

fn empty_view<'a>() -> Element<'a, Message> {
    column![
        text("Inbox zero")
            .size(style::FONT_SIZE_NORMAL)
            .color(style::M3_ON_SURFACE),
        text("No pending GitLab todos")
            .size(style::FONT_SIZE_SMALL)
            .color(style::M3_ON_SURFACE_VARIANT),
    ]
    .spacing(style::SPACING_SMALL)
    .into()
}

fn list_view(info: &GitlabInfo) -> Element<'_, Message> {
    let mut list = column![].spacing(2.0).width(Length::Fill);

    let max = style::GITLAB_PANEL_VISIBLE;
    for item in info.todos.iter().take(max) {
        list = list.push(todo_entry(item));
    }

    if info.total > max {
        let extra = info.total.saturating_sub(max);
        list = list.push(
            container(
                text(format!("+{extra} more — use \"Show all\""))
                    .size(style::FONT_SIZE_SMALL)
                    .color(style::M3_ON_SURFACE_VARIANT),
            )
            .padding(style::PADDING_ENTRY),
        );
    }

    scrollable(list)
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::new()
                .width(6.0)
                .scroller_width(6.0)
                .spacing(style::SPACING_SMALL),
        ))
        .style(scrollable_style)
        .height(Length::Fill)
        .into()
}

#[allow(clippy::too_many_lines)]
pub fn view(info: &GitlabInfo) -> Element<'_, Message> {
    let header = row![
        text(style::ICON_TASK_ALT)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_TERTIARY),
        text("GitLab todos")
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_ON_SURFACE),
        Space::new().width(Length::Fill),
        text(if info.total == 0 {
            String::new()
        } else {
            info.total.to_string()
        })
        .size(style::FONT_SIZE_NORMAL)
        .color(style::M3_ON_SURFACE_VARIANT),
    ]
    .spacing(style::SPACING_SMALLER)
    .align_y(Alignment::Center);

    let body: Element<'_, Message> = match info.auth {
        AuthState::Missing | AuthState::Invalid => auth_setup_view(info),
        AuthState::Authenticated if info.todos.is_empty() => empty_view(),
        AuthState::Authenticated => list_view(info),
    };

    let show_all_url = format!("{}{}", info.host, TODO_PAGE_PATH);
    let footer = row![
        Space::new().width(Length::Fill),
        pill_action_button(
            style::ICON_OPEN_IN_NEW,
            "Show all",
            Message::GitlabOpenUrl(show_all_url),
        ),
    ]
    .align_y(Alignment::Center);

    let content = column![header, separator(), body, separator(), footer]
        .spacing(style::SPACING_NORMAL)
        .width(Length::Fill)
        .height(Length::Fill);

    let panel = container(content)
        .padding(style::PADDING_LARGE)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(style::audio_panel_container);

    panel_with_exit(panel.into())
}
