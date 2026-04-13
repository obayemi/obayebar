mod bar;
mod notifications;
mod services;
mod style;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use bar::workspaces::SpringState;
use iced::widget::canvas;
use iced::window;
use iced::{Color, Element, Subscription, Task, Theme};
use iced_layershell::reexport::{
    Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings, OutputOption,
};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::to_layer_message;
use notifications::{NotifEvent, NotificationData};
use services::audio::{AudioCommand, AudioInfo};
use services::battery::BatteryInfo;
use services::hyprland::{HyprEvent, HyprState, WindowInfo, WorkspaceInfo};
use services::network::NetworkInfo;
use services::tray::TrayItemInfo;

/// A logger wrapper that exits the process on fatal Wayland protocol errors,
/// since layershellev silently swallows them and keeps the event loop running.
struct FatalErrorLogger {
    inner: env_logger::Logger,
}

static WAYLAND_FATAL: AtomicBool = AtomicBool::new(false);

impl log::Log for FatalErrorLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if self.inner.enabled(record.metadata()) {
            self.inner.log(record);
        }

        // Detect fatal Wayland protocol errors and exit on first occurrence
        if record.level() == log::Level::Error
            && record.target().starts_with("wayland_backend")
            && !WAYLAND_FATAL.swap(true, Ordering::Relaxed)
        {
            eprintln!("Fatal Wayland error, exiting.");
            std::process::exit(1);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

fn main() {
    let logger = env_logger::Builder::from_default_env().build();
    let max_level = logger.filter();
    log::set_boxed_logger(Box::new(FatalErrorLogger { inner: logger }))
        .map(|()| log::set_max_level(max_level))
        .ok();

    let icon_fonts = style::load_icon_font();

    // The initial window is created by settings on the default output.
    // Additional monitors get windows via NewLayerShell in setup_bars().
    let result = iced_layershell::daemon(App::new, App::namespace, App::update, App::view)
        .settings(Settings {
            layer_settings: LayerShellSettings {
                anchor: Anchor::Left | Anchor::Top | Anchor::Bottom,
                layer: Layer::Top,
                exclusive_zone: i32::try_from(style::BAR_WIDTH).unwrap_or(54),
                size: Some((style::BAR_WIDTH, 0)),
                keyboard_interactivity: KeyboardInteractivity::None,
                ..LayerShellSettings::default()
            },
            fonts: icon_fonts,
            ..Settings::default()
        })
        .subscription(App::subscription)
        .theme(theme_fn)
        .run();

    if let Err(err) = result {
        log::error!("obayebar exiting: {err}");
        std::process::exit(1);
    }
}

#[derive(Debug)]
pub struct App {
    /// Map of bar window ID -> monitor name (for extra monitors only)
    extra_bar_windows: HashMap<window::Id, String>,
    /// The monitor that the initial (settings-created) window is on
    initial_monitor: Option<String>,
    /// Set of monitors that already have bars
    monitors_with_bars: Vec<String>,
    /// Per-monitor workspace indicator spring animation
    ws_spring: HashMap<String, SpringState>,
    /// Per-monitor workspace canvas cache (cleared on data change)
    pub ws_cache: HashMap<String, canvas::Cache>,
    /// Fallback cache used before monitor-specific caches are created
    pub ws_cache_fallback: canvas::Cache,
    /// Vector font for canvas text rendering
    pub vector_font: Option<ab_glyph::FontArc>,

    notif_popup_id: Option<window::Id>,
    notif_center_id: Option<window::Id>,
    audio_panel_id: Option<window::Id>,
    audio_panel_open: bool,
    network_panel_id: Option<window::Id>,
    network_panel_open: bool,

    pub workspaces: Vec<WorkspaceInfo>,
    /// Per-monitor active workspace: `monitor_name` -> `active_workspace_id`
    pub active_workspaces: HashMap<String, i32>,
    pub active_window: Option<WindowInfo>,
    pub time: chrono::DateTime<chrono::Local>,
    pub battery: BatteryInfo,
    pub network: NetworkInfo,
    pub audio: AudioInfo,
    pub tray_items: Vec<TrayItemInfo>,
    pub notifications: Vec<NotificationData>,
    pub popup_notifications: Vec<NotificationData>,
    pub notification_center_open: bool,
}

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    AnimTick,
    Hyprland(HyprEvent),
    WorkspaceClick(i32),
    Battery(BatteryInfo),
    Network(NetworkInfo),
    Audio(AudioInfo),
    TrayItems(Vec<TrayItemInfo>),
    TrayClick(String),
    Notif(NotifEvent),
    NotifDismiss(u32),
    NotifToggleExpand(u32),
    NotifCenterToggle,
    NotifClearAll,
    PowerClick,
    AudioPanelOpen(Option<String>),
    NetworkPanelOpen(Option<String>),
    CloseAllPanels,
    AudioSetVolume(f32),
    AudioSetMute(bool),
    AudioSetDefaultSink(u32),
    AudioOpenPavucontrol,
}

impl App {
    fn new() -> (Self, Task<Message>) {
        (
            Self {
                extra_bar_windows: HashMap::new(),
                initial_monitor: None,
                monitors_with_bars: Vec::new(),
                ws_spring: HashMap::new(),
                ws_cache: HashMap::new(),
                ws_cache_fallback: canvas::Cache::default(),
                vector_font: style::load_vector_font(),
                notif_popup_id: None,
                notif_center_id: None,
                audio_panel_id: None,
                audio_panel_open: false,
                network_panel_id: None,
                network_panel_open: false,
                workspaces: Vec::new(),
                active_workspaces: HashMap::new(),
                active_window: None,
                time: chrono::Local::now(),
                battery: BatteryInfo::default(),
                network: NetworkInfo::default(),
                audio: AudioInfo::default(),
                tray_items: Vec::new(),
                notifications: Vec::new(),
                popup_notifications: Vec::new(),
                notification_center_open: false,
            },
            Task::none(),
        )
    }

    fn namespace() -> String {
        "obayebar".into()
    }

    /// Get the monitor name for a bar window ID
    fn monitor_for_bar(&self, id: window::Id) -> Option<&str> {
        self.extra_bar_windows
            .get(&id)
            .map(String::as_str)
            .or(self.initial_monitor.as_deref())
    }

    /// Get the active workspace ID for a `monitor`
    #[must_use]
    pub fn active_workspace_for_monitor(&self, monitor: &str) -> i32 {
        self.active_workspaces.get(monitor).copied().unwrap_or(1)
    }

    /// Get workspaces for a specific `monitor`
    #[must_use]
    pub fn workspaces_for_monitor(&self, monitor: &str) -> Vec<&WorkspaceInfo> {
        self.workspaces
            .iter()
            .filter(|w| w.monitor == monitor)
            .collect()
    }

    #[allow(clippy::too_many_lines)]
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                self.time = chrono::Local::now();
                self.expire_popups()
            }
            Message::AnimTick => {
                let dt = 1.0 / 60.0;
                for (monitor, spring) in &mut self.ws_spring {
                    if spring.tick(dt) {
                        if let Some(cache) = self.ws_cache.get(monitor) {
                            cache.clear();
                        }
                    }
                }
                Task::none()
            }
            Message::Hyprland(event) => match event {
                HyprEvent::State(state) => self.apply_hypr_state(state),
                HyprEvent::ActiveWindow(win) => {
                    self.active_window = win;
                    Task::none()
                }
            },
            Message::WorkspaceClick(id) => {
                services::hyprland::switch_workspace(id);
                Task::none()
            }
            Message::Battery(info) => {
                self.battery = info;
                Task::none()
            }
            Message::Network(info) => {
                self.network = info;
                Task::none()
            }
            Message::Audio(info) => {
                self.audio = info;
                Task::none()
            }
            Message::TrayItems(items) => {
                self.tray_items = items;
                Task::none()
            }
            Message::TrayClick(id) => {
                services::tray::activate_item(&id);
                Task::none()
            }
            Message::Notif(event) => match event {
                NotifEvent::Received(notif) => {
                    self.notifications.retain(|n| n.id != notif.id);
                    self.popup_notifications.retain(|n| n.id != notif.id);
                    self.notifications.insert(0, notif.clone());
                    self.popup_notifications.insert(0, notif);
                    self.ensure_popup_window()
                }
                NotifEvent::Closed(id) => {
                    self.popup_notifications.retain(|n| n.id != id);
                    self.notifications.retain(|n| n.id != id);
                    self.maybe_close_popup_window()
                }
            },
            Message::NotifDismiss(id) => {
                self.popup_notifications.retain(|n| n.id != id);
                self.notifications.retain(|n| n.id != id);
                self.maybe_close_popup_window()
            }
            Message::NotifToggleExpand(id) => {
                for notif in &mut self.notifications {
                    if notif.id == id {
                        notif.expanded = !notif.expanded;
                    }
                }
                for notif in &mut self.popup_notifications {
                    if notif.id == id {
                        notif.expanded = !notif.expanded;
                    }
                }
                Task::none()
            }
            Message::NotifCenterToggle | Message::PowerClick => self.toggle_notif_center(),
            Message::NotifClearAll => {
                self.notifications.clear();
                self.popup_notifications.clear();
                let mut tasks = vec![self.maybe_close_popup_window()];
                if let Some(id) = self.notif_center_id.take() {
                    self.notification_center_open = false;
                    tasks.push(close_window(id));
                }
                Task::batch(tasks)
            }
            Message::AudioPanelOpen(monitor) => {
                let close = self.close_all_panels();
                let open = self.open_audio_panel(monitor);
                Task::batch([close, open])
            }
            Message::NetworkPanelOpen(monitor) => {
                let close = self.close_all_panels();
                let open = self.open_network_panel(monitor);
                Task::batch([close, open])
            }
            Message::CloseAllPanels => self.close_all_panels(),
            Message::AudioSetVolume(vol) => {
                services::audio::send_command(AudioCommand::Volume(vol));
                Task::none()
            }
            Message::AudioSetMute(muted) => {
                services::audio::send_command(AudioCommand::Mute(muted));
                Task::none()
            }
            Message::AudioSetDefaultSink(id) => {
                services::audio::send_command(AudioCommand::DefaultSink { id });
                Task::none()
            }
            Message::AudioOpenPavucontrol => {
                tokio::spawn(async {
                    let _ = tokio::process::Command::new("pavucontrol").spawn();
                });
                Task::none()
            }
            _ => Task::none(),
        }
    }

    /// Apply a full Hyprland state update. Creates bar windows for new monitors.
    fn apply_hypr_state(&mut self, state: HyprState) -> Task<Message> {
        self.workspaces = state.workspaces;
        self.active_window = state.active_window;

        // Invalidate all workspace caches since data changed
        for cache in self.ws_cache.values() {
            cache.clear();
        }

        // Update spring targets for each monitor's active workspace
        for (monitor, &active_ws_id) in &state.active_workspaces {
            let mut sorted_ids: Vec<i32> = self
                .workspaces
                .iter()
                .filter(|w| &w.monitor == monitor && w.id > 0 && !w.name.starts_with("special:"))
                .map(|w| w.id)
                .collect();
            sorted_ids.sort_unstable();

            #[allow(clippy::cast_precision_loss)]
            let target = sorted_ids
                .iter()
                .position(|&id| id == active_ws_id)
                .unwrap_or(0) as f32;

            self.ws_cache.entry(monitor.clone()).or_default();
            let spring = self.ws_spring.entry(monitor.clone()).or_default();
            if spring.position == 0.0 && spring.target == 0.0 && target != 0.0 {
                // First time seeing this monitor — snap to position
                spring.snap(target);
            } else {
                spring.set_target(target);
            }
        }

        self.active_workspaces = state.active_workspaces;

        // The initial settings window lands on the focused monitor.
        // Assign it on first state update.
        if self.initial_monitor.is_none() {
            self.initial_monitor = Some(state.focused_monitor.clone());
            self.monitors_with_bars.push(state.focused_monitor);
        }

        // Create bars for any monitors we haven't seen yet
        let mut tasks = Vec::new();
        for monitor in state.monitors {
            if self.monitors_with_bars.contains(&monitor) {
                continue;
            }
            self.monitors_with_bars.push(monitor.clone());

            let id = window::Id::unique();
            self.extra_bar_windows.insert(id, monitor.clone());
            tasks.push(Task::done(Message::NewLayerShell {
                settings: NewLayerShellSettings {
                    anchor: Anchor::Left | Anchor::Top | Anchor::Bottom,
                    layer: Layer::Top,
                    exclusive_zone: Some(i32::try_from(style::BAR_WIDTH).unwrap_or(54)),
                    size: Some((style::BAR_WIDTH, 0)),
                    output_option: OutputOption::OutputName(monitor),
                    keyboard_interactivity: KeyboardInteractivity::None,
                    ..NewLayerShellSettings::default()
                },
                id,
            }));
        }

        Task::batch(tasks)
    }

    fn view(&self, id: window::Id) -> Element<'_, Message> {
        if Some(id) == self.notif_popup_id {
            notifications::popup_view(self)
        } else if Some(id) == self.notif_center_id {
            notifications::center_view(self)
        } else if Some(id) == self.audio_panel_id {
            bar::audio_panel::view(&self.audio)
        } else if Some(id) == self.network_panel_id {
            bar::network_panel::view(&self.network)
        } else {
            let monitor = self.monitor_for_bar(id);
            bar::view(self, monitor)
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let is_animating = self.ws_spring.values().any(SpringState::is_animating);

        let mut subs = vec![
            iced::time::every(std::time::Duration::from_secs(1)).map(|_| Message::Tick),
            Subscription::run(services::hyprland::stream).map(Message::Hyprland),
            Subscription::run(services::battery::stream).map(Message::Battery),
            Subscription::run(services::network::stream).map(Message::Network),
            Subscription::run(services::audio::stream).map(Message::Audio),
            Subscription::run(services::tray::stream).map(Message::TrayItems),
            Subscription::run(notifications::daemon::stream).map(Message::Notif),
        ];

        if is_animating {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(16)).map(|_| Message::AnimTick),
            );
        }

        Subscription::batch(subs)
    }

    fn toggle_notif_center(&mut self) -> Task<Message> {
        self.notification_center_open = !self.notification_center_open;
        if self.notification_center_open {
            let id = window::Id::unique();
            self.notif_center_id = Some(id);
            Task::done(Message::NewLayerShell {
                settings: NewLayerShellSettings {
                    anchor: Anchor::Right | Anchor::Top | Anchor::Bottom,
                    layer: Layer::Overlay,
                    exclusive_zone: Some(-1),
                    size: Some((style::NOTIF_WIDTH, 0)),
                    margin: Some((8, 8, 8, 8)),
                    keyboard_interactivity: KeyboardInteractivity::None,
                    ..NewLayerShellSettings::default()
                },
                id,
            })
        } else if let Some(id) = self.notif_center_id.take() {
            close_window(id)
        } else {
            Task::none()
        }
    }

    fn close_all_panels(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        if self.audio_panel_open {
            self.audio_panel_open = false;
            if let Some(id) = self.audio_panel_id.take() {
                tasks.push(close_window(id));
            }
        }
        if self.network_panel_open {
            self.network_panel_open = false;
            if let Some(id) = self.network_panel_id.take() {
                tasks.push(close_window(id));
            }
        }
        Task::batch(tasks)
    }

    fn open_audio_panel(&mut self, monitor: Option<String>) -> Task<Message> {
        if self.audio_panel_open {
            return Task::none();
        }
        self.audio_panel_open = true;
        let id = window::Id::unique();
        self.audio_panel_id = Some(id);
        let output_option = monitor.map_or(OutputOption::LastOutput, OutputOption::OutputName);
        let panel_height = style::audio_panel_height(self.audio.sinks.len());
        Task::done(Message::NewLayerShell {
            settings: NewLayerShellSettings {
                anchor: Anchor::Left | Anchor::Bottom,
                layer: Layer::Overlay,
                exclusive_zone: Some(-1),
                size: Some((style::AUDIO_PANEL_WIDTH, panel_height)),
                margin: Some((0, 0, 8, (style::BAR_WIDTH + 8).cast_signed())),
                keyboard_interactivity: KeyboardInteractivity::None,
                output_option,
                ..NewLayerShellSettings::default()
            },
            id,
        })
    }

    fn open_network_panel(&mut self, monitor: Option<String>) -> Task<Message> {
        if self.network_panel_open {
            return Task::none();
        }
        self.network_panel_open = true;
        let id = window::Id::unique();
        self.network_panel_id = Some(id);
        let output_option = monitor.map_or(OutputOption::LastOutput, OutputOption::OutputName);
        let ap_count = self.network.access_points.len().clamp(1, 8);
        let panel_height = style::network_panel_height(ap_count);
        Task::done(Message::NewLayerShell {
            settings: NewLayerShellSettings {
                anchor: Anchor::Left | Anchor::Bottom,
                layer: Layer::Overlay,
                exclusive_zone: Some(-1),
                size: Some((style::NETWORK_PANEL_WIDTH, panel_height)),
                margin: Some((0, 0, 8, (style::BAR_WIDTH + 8).cast_signed())),
                keyboard_interactivity: KeyboardInteractivity::None,
                output_option,
                ..NewLayerShellSettings::default()
            },
            id,
        })
    }

    fn expire_popups(&mut self) -> Task<Message> {
        let now = chrono::Local::now();
        self.popup_notifications
            .retain(|n| n.expire_at.is_none_or(|exp| now < exp));
        self.maybe_close_popup_window()
    }

    fn ensure_popup_window(&mut self) -> Task<Message> {
        if self.popup_notifications.is_empty() {
            return Task::none();
        }
        let height = style::notif_popup_height(self.popup_notifications.len());
        if let Some(id) = self.notif_popup_id {
            // Resize existing window to fit current notification count
            return Task::done(Message::SizeChange {
                id,
                size: (style::NOTIF_WIDTH, height),
            });
        }
        let id = window::Id::unique();
        self.notif_popup_id = Some(id);
        Task::done(Message::NewLayerShell {
            settings: NewLayerShellSettings {
                anchor: Anchor::Right | Anchor::Top,
                layer: Layer::Overlay,
                exclusive_zone: Some(-1),
                size: Some((style::NOTIF_WIDTH, height)),
                margin: Some((8, 8, 8, 8)),
                keyboard_interactivity: KeyboardInteractivity::None,
                ..NewLayerShellSettings::default()
            },
            id,
        })
    }

    fn maybe_close_popup_window(&mut self) -> Task<Message> {
        if self.popup_notifications.is_empty() {
            if let Some(id) = self.notif_popup_id.take() {
                return close_window(id);
            }
            return Task::none();
        }
        // Resize to fit remaining notifications
        self.ensure_popup_window()
    }
}

fn theme_fn(_app: &App, _id: window::Id) -> Theme {
    Theme::custom(
        String::from("obayebar-dark"),
        iced::theme::Palette {
            background: Color::TRANSPARENT,
            text: style::M3_ON_SURFACE,
            primary: style::M3_PRIMARY,
            success: Color::from_rgb(0.2, 0.8, 0.2),
            danger: style::M3_ERROR,
            warning: style::M3_TERTIARY,
        },
    )
}

fn close_window(id: window::Id) -> Task<Message> {
    iced_runtime::task::effect(iced_runtime::Action::Window(
        iced_runtime::window::Action::Close(id),
    ))
}
