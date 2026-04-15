mod active_window;
pub mod audio_panel;
pub mod battery_panel;
pub mod bluetooth_panel;
mod clock;
mod logo;
pub mod network_panel;
mod status;
pub mod sysinfo_panel;
mod tray;
pub mod workspaces;

use crate::style;
use crate::App;
use crate::Message;
use chrono::{Datelike, Timelike};
use iced::widget::{column, container, lazy, Space};
use iced::{Background, Element, Length, Padding};

/// Bucket a percentage into threshold categories for cache-key hashing.
/// 0 = normal, 1 = elevated (>=70%), 2 = critical (>=90%)
#[allow(clippy::arithmetic_side_effects)]
fn usage_bucket(percent: f32) -> u8 {
    u8::from(percent >= 90.0) + u8::from(percent >= 70.0)
}

/// Build a hashable cache key for the status icon section.
/// Only includes values that affect the rendered output.
#[allow(clippy::type_complexity)]
fn status_cache_key(
    app: &App,
    monitor: Option<&str>,
) -> (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    bool,
    bool,
    (u8, u8, u8),
    Option<String>,
) {
    (
        app.audio.icon_name,
        app.network.icon_name,
        app.bluetooth.icon_name,
        app.battery.icon_name,
        app.battery.present,
        app.battery.percentage <= 20.0,
        (
            usage_bucket(app.sysinfo.cpu_percent),
            usage_bucket(app.sysinfo.gpu_percent),
            usage_bucket(app.sysinfo.ram_percent),
        ),
        monitor.map(String::from),
    )
}

pub fn view<'a>(app: &'a App, monitor: Option<&'a str>) -> Element<'a, Message> {
    let monitor_workspaces: Vec<&crate::services::hyprland::WorkspaceInfo> = monitor.map_or_else(
        || app.workspaces.iter().collect(),
        |m| app.workspaces_for_monitor(m),
    );

    let active_ws = monitor.map_or(1, |m| app.active_workspace_for_monitor(m));
    let default_spring = workspaces::SpringState::default();
    let spring = monitor
        .and_then(|m| app.ws_spring.get(m))
        .unwrap_or(&default_spring);
    let ws_cache = monitor
        .and_then(|m| app.ws_cache.get(m))
        .unwrap_or(&app.ws_cache_fallback);

    // Cache keys for lazy sections
    let active_title: Option<String> = app.active_window.as_ref().map(|w| w.title.clone());
    let has_font = app.vector_font.is_some();

    let tray_items = app.tray_items.clone();

    let time = app.time;
    let clock_key = (time.hour(), time.minute(), time.day());

    let status_key = status_cache_key(app, monitor);
    let battery = app.battery.clone();
    let network = app.network.clone();
    let audio = app.audio.clone();
    let bluetooth = app.bluetooth.clone();
    let sysinfo = app.sysinfo.clone();
    let monitor_owned = monitor.map(String::from);

    let active_font = app.vector_font.clone();
    let active_window = app.active_window.clone();

    let bar_content = column![
        lazy((), |()| { logo::view() }),
        workspaces::view(&monitor_workspaces, active_ws, spring, ws_cache),
        Space::new().width(Length::Shrink).height(Length::Fill),
        lazy((active_title, has_font), move |_| {
            active_window::view(active_window.as_ref(), active_font.as_ref())
        }),
        Space::new().width(Length::Shrink).height(Length::Fill),
        lazy(tray_items, |items| { tray::view(items) }),
        lazy(clock_key, move |_| { clock::view(&time) }),
        lazy(status_key, move |_| {
            let monitor_ref = monitor_owned.as_deref();
            status::view(&battery, &network, &audio, &bluetooth, &sysinfo, monitor_ref)
        }),
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
    let bar = container(bar_content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(Background::Color(style::with_alpha(
                style::M3_SURFACE,
                0.85,
            ))),
            ..container::Style::default()
        });

    bar.into()
}
