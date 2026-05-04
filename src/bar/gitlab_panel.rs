use super::widgets::{hover_button_style, panel_with_exit, separator};
use crate::services::gitlab::{self, AuthState, GitlabInfo, TodoItem, TODO_PAGE_PATH};
use crate::Message;
use iced::widget::{button, column, container, row, scrollable, text, text_input, Space};
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
    let title: &str = if item.title.is_empty() {
        "(no title)"
    } else {
        &item.title
    };

    let header = row![
        text(action_label)
            .size(style::FONT_SIZE_SMALL)
            .color(style::M3_TERTIARY),
        Space::new().width(Length::Fill),
        text(item.project.as_str())
            .size(style::FONT_SIZE_SMALL)
            .color(style::M3_ON_SURFACE_VARIANT),
    ]
    .spacing(style::SPACING_SMALL)
    .align_y(Alignment::Center);

    let body = text(title)
        .size(style::FONT_SIZE_NORMAL)
        .color(style::M3_ON_SURFACE);

    let content = column![header, body].spacing(2.0).width(Length::Fill);

    button(content)
        .on_press(Message::GitlabOpenUrl(item.url.clone()))
        .style(hover_button_style(Color::TRANSPARENT, style::M3_ON_SURFACE))
        .padding(style::PADDING_ENTRY)
        .width(Length::Fill)
        .into()
}

fn pill_action_button<'a>(
    icon: &'a str,
    label: &'a str,
    msg: Option<Message>,
) -> Element<'a, Message> {
    let enabled = msg.is_some();
    let text_color = if enabled {
        style::M3_ON_SURFACE
    } else {
        style::M3_ON_SURFACE_VARIANT
    };
    let row = row![
        text(icon)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_NORMAL)
            .color(text_color),
        text(label).size(style::FONT_SIZE_NORMAL).color(text_color),
    ]
    .spacing(style::SPACING_SMALL)
    .align_y(Alignment::Center);

    let mut btn = button(row)
        .style(hover_button_style(
            style::with_alpha(style::M3_SURFACE_CONTAINER_HIGH, 0.9),
            text_color,
        ))
        .padding(style::PADDING_ENTRY);
    if let Some(m) = msg {
        btn = btn.on_press(m);
    }
    btn.into()
}

fn token_input_row<'a>(value: &str) -> Element<'a, Message> {
    let input = text_input("Tap the paste icon →", value)
        .on_input(Message::GitlabTokenInputChanged)
        .on_submit(Message::GitlabTokenSubmit)
        .secure(true)
        .size(style::FONT_SIZE_NORMAL)
        .padding(style::PADDING_SMALL)
        .width(Length::Fill)
        .style(|_theme, status| {
            let border_color = match status {
                text_input::Status::Focused { .. } => style::M3_PRIMARY,
                text_input::Status::Hovered => style::M3_ON_SURFACE_VARIANT,
                _ => style::M3_OUTLINE_VARIANT,
            };
            text_input::Style {
                background: Background::Color(style::with_alpha(style::M3_SURFACE_CONTAINER, 0.95)),
                border: Border {
                    radius: style::ROUNDING_EXTRA_SMALL.into(),
                    width: 1.0,
                    color: border_color,
                },
                icon: style::M3_ON_SURFACE_VARIANT,
                placeholder: style::M3_ON_SURFACE_VARIANT,
                value: style::M3_ON_SURFACE,
                selection: style::with_alpha(style::M3_PRIMARY, 0.3),
            }
        });

    let paste = button(
        text(style::ICON_CONTENT_PASTE)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_NORMAL)
            .color(style::M3_ON_SURFACE),
    )
    .on_press(Message::GitlabTokenInputPaste)
    .style(hover_button_style(
        style::with_alpha(style::M3_SURFACE_CONTAINER_HIGH, 0.9),
        style::M3_ON_SURFACE,
    ))
    .padding(style::PADDING_SMALL);

    row![input, paste]
        .spacing(style::SPACING_SMALL)
        .align_y(Alignment::Center)
        .into()
}

/// Read the kernel hostname; falls back to a blank string when unavailable.
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Build the GitLab "create token" URL, prefilled with a host-scoped name and
/// a one-year expiry. The token name uses `obayebar-<hostname>` so multiple
/// machines authorized for the same account remain distinguishable.
fn create_token_url() -> String {
    let host = hostname();
    let name = if host.is_empty() {
        "obayebar".to_string()
    } else {
        format!("obayebar-{host}")
    };
    let host_url = crate::services::gitlab::host();
    let base =
        format!("{host_url}/-/user_settings/personal_access_tokens?name={name}&scopes=read_api");
    match chrono::Local::now()
        .date_naive()
        .checked_add_months(chrono::Months::new(12))
    {
        Some(d) => format!("{base}&expires_at={}", d.format("%Y-%m-%d")),
        None => base,
    }
}

fn auth_setup_view<'a>(info: &'a GitlabInfo, token_input: &str) -> Element<'a, Message> {
    let title = if matches!(info.auth, AuthState::Invalid) {
        "GitLab token rejected"
    } else {
        "GitLab token not configured"
    };
    let intro = text(title)
        .size(style::FONT_SIZE_NORMAL)
        .color(style::M3_ON_SURFACE);

    let instructions = text(
        "1. Create a Personal Access Token with the read_api scope on GitLab and copy it.\n2. Hit the paste icon to pull it from the clipboard, then Submit.\n\nThe token is saved to the system keyring when one is running, or to a 0600 file otherwise.",
    )
    .size(style::FONT_SIZE_SMALL)
    .color(style::M3_ON_SURFACE_VARIANT);

    let create_url = create_token_url();

    let submit_disabled = token_input.trim().is_empty();
    let submit = pill_action_button(
        style::ICON_CHECK_CIRCLE,
        "Submit",
        if submit_disabled {
            None
        } else {
            Some(Message::GitlabTokenSubmit)
        },
    );

    let buttons = column![
        pill_action_button(
            style::ICON_KEY,
            "Create access token on gitlab",
            Some(Message::GitlabOpenUrl(create_url)),
        ),
        token_input_row(token_input),
        submit,
        pill_action_button(
            style::ICON_FOLDER,
            "Open token file",
            Some(Message::GitlabOpenTokenFile),
        ),
        pill_action_button(
            style::ICON_REFRESH,
            "Reload token",
            Some(Message::GitlabReloadToken),
        ),
        pill_action_button(
            style::ICON_DELETE,
            "Forget token & restart",
            Some(Message::GitlabForgetToken),
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
pub fn view<'a>(info: &'a GitlabInfo, token_input: &'a str) -> Element<'a, Message> {
    let mut header = row![
        text(style::ICON_TASK_ALT)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_TERTIARY),
        text("GitLab todos")
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_ON_SURFACE),
        Space::new().width(Length::Fill),
    ]
    .spacing(style::SPACING_SMALLER)
    .align_y(Alignment::Center);

    if info.total > 0 {
        header = header.push(
            text(info.total.to_string())
                .size(style::FONT_SIZE_NORMAL)
                .color(style::M3_ON_SURFACE_VARIANT),
        );
    }

    let body: Element<'a, Message> = match info.auth {
        AuthState::Missing | AuthState::Invalid => auth_setup_view(info, token_input),
        AuthState::Authenticated if info.todos.is_empty() => empty_view(),
        AuthState::Authenticated => list_view(info),
    };

    let show_all_url = format!("{}{}", crate::services::gitlab::host(), TODO_PAGE_PATH);
    let footer = row![
        Space::new().width(Length::Fill),
        pill_action_button(
            style::ICON_OPEN_IN_NEW,
            "Show all",
            Some(Message::GitlabOpenUrl(show_all_url)),
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
