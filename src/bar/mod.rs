mod active_window;
mod clock;
mod logo;
mod power;
mod status;
mod tray;
pub mod workspaces;

use crate::style;
use crate::App;
use crate::Message;
use iced::widget::{column, container, Space};
use iced::{Background, Element, Length, Padding};

pub fn view<'a>(app: &'a App, monitor: Option<&'a str>) -> Element<'a, Message> {
    let monitor_workspaces: Vec<&crate::services::hyprland::WorkspaceInfo> = monitor.map_or_else(
        || app.workspaces.iter().collect(),
        |m| app.workspaces_for_monitor(m),
    );

    let active_ws = monitor.map_or(1, |m| app.active_workspace_for_monitor(m));
    let default_anim = workspaces::AnimState::default();
    let anim = monitor
        .and_then(|m| app.ws_anim.get(m))
        .unwrap_or(&default_anim);

    let bar_content = column![
        logo::view(),
        workspaces::view(&monitor_workspaces, active_ws, anim),
        Space::new().width(Length::Shrink).height(Length::Fill),
        active_window::view(app.active_window.as_ref()),
        Space::new().width(Length::Shrink).height(Length::Fill),
        tray::view(&app.tray_items),
        clock::view(&app.time),
        status::view(&app.battery, &app.network, &app.audio),
        power::view(),
    ]
    .spacing(style::SPACING_NORMAL)
    .padding(Padding {
        top: style::PADDING_LARGE,
        bottom: style::PADDING_LARGE,
        left: style::BAR_PADDING,
        right: style::BAR_PADDING,
    })
    .align_x(iced::Alignment::Center)
    .width(Length::Fill)
    .height(Length::Fill);

    // Caelestia uses m3surface background with transparency.base (0.85) alpha
    container(bar_content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(Background::Color(style::with_alpha(
                style::M3_SURFACE,
                0.85,
            ))),
            ..container::Style::default()
        })
        .into()
}
