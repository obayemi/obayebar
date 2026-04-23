mod bar;
mod notifications;
mod panel;
mod services;

use obayebar::style;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use bar::workspaces::SpringState;
use iced::widget::canvas;
use iced::window;
use iced::{Element, Subscription, Task, Theme};
use iced_layershell::reexport::{
    Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings, OutputOption,
};
use iced_layershell::settings::{LayerShellSettings, Settings};
use iced_layershell::to_layer_message;
use services::audio::{AudioCommand, AudioInfo};
use services::battery::BatteryInfo;
use services::bluetooth::BluetoothInfo;
use services::hyprland::{HyprEvent, HyprState, WindowInfo, WorkspaceInfo};
use services::network::NetworkInfo;
use services::notifications::{NotifEvent, NotificationData};
use services::sysinfo::SysInfo;
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

    // The bar has no text input and doesn't need clipboard. The forked
    // iced_layershell (vendor/iced_layershell) exposes this opt-out to
    // skip spawning the always-on smithay-clipboard worker thread.
    iced_layershell::disable_clipboard();

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
            antialiasing: true,
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
#[allow(clippy::struct_excessive_bools)]
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
    audio_panel: panel::Panel,
    network_panel: panel::Panel,
    battery_panel: panel::Panel,
    bluetooth_panel: panel::Panel,
    sysinfo_panel: panel::Panel,

    pub workspaces: Vec<WorkspaceInfo>,
    /// Per-monitor active workspace: `monitor_name` -> `active_workspace_id`
    pub active_workspaces: HashMap<String, i32>,
    pub active_window: Option<WindowInfo>,
    pub time: chrono::DateTime<chrono::Local>,
    pub battery: BatteryInfo,
    pub network: NetworkInfo,
    pub connecting_ssid: Option<String>,
    pub audio: AudioInfo,
    pub bluetooth: BluetoothInfo,
    pub sysinfo: SysInfo,
    pub tray_items: Vec<TrayItemInfo>,
    pub popup_notifications: Vec<NotificationData>,
    pub hovered_notif_id: Option<u32>,
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
    SysInfo(SysInfo),
    Audio(AudioInfo),
    TrayItems(Vec<TrayItemInfo>),
    TrayClick(String),
    Notif(NotifEvent),
    NotifDismiss(u32),
    NotifActivate(u32),
    NotifHoverEnter(u32),
    NotifHoverExit(u32),
    AudioPanelOpen(Option<String>),
    NetworkPanelOpen(Option<String>),
    BatteryPanelOpen(Option<String>),
    BluetoothPanelOpen(Option<String>),
    SysinfoPanelOpen(Option<String>),
    Bluetooth(BluetoothInfo),
    BluetoothToggleDevice { path: String, connected: bool },
    BluetoothSetPowered(bool),
    BluetoothSetDiscovery(bool),
    BluetoothForgetDevice(String),
    NetworkSetWifiEnabled(bool),
    NetworkConnect(String),
    NetworkDisconnect,
    CloseAllPanels,
    AudioSetVolume(f32),
    AudioSetMute(bool),
    AudioSetDefaultSink(u32),
    AudioOpenPavucontrol,
    SetPowerProfile(String),
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
                audio_panel: panel::Panel::new(),
                network_panel: panel::Panel::new(),
                battery_panel: panel::Panel::new(),
                bluetooth_panel: panel::Panel::new(),
                sysinfo_panel: panel::Panel::new(),
                workspaces: Vec::new(),
                active_workspaces: HashMap::new(),
                active_window: None,
                time: chrono::Local::now(),
                battery: BatteryInfo::default(),
                network: NetworkInfo::default(),
                connecting_ssid: None,
                audio: AudioInfo::default(),
                bluetooth: BluetoothInfo::default(),
                sysinfo: SysInfo::default(),
                tray_items: Vec::new(),
                popup_notifications: Vec::new(),
                hovered_notif_id: None,
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
                if self.battery != info {
                    self.battery = info;
                }
                Task::none()
            }
            Message::Network(info) => {
                // Clear connecting state when connection changes
                if let Some(ref ssid) = self.connecting_ssid {
                    if info.wifi_ssid.as_deref() == Some(ssid) || !info.wifi {
                        self.connecting_ssid = None;
                    }
                }
                if self.network != info {
                    self.network = info;
                }
                Task::none()
            }
            Message::Audio(info) => {
                if self.audio != info {
                    self.audio = info;
                }
                Task::none()
            }
            Message::Bluetooth(info) => {
                if self.bluetooth != info {
                    self.bluetooth = info;
                }
                Task::none()
            }
            Message::SysInfo(info) => {
                if self.sysinfo != info {
                    self.sysinfo = info;
                }
                Task::none()
            }
            Message::TrayItems(items) => {
                if self.tray_items != items {
                    self.tray_items = items;
                }
                Task::none()
            }
            Message::TrayClick(id) => {
                services::tray::activate_item(&id);
                Task::none()
            }
            Message::Notif(event) => match event {
                NotifEvent::Received(notif) => {
                    self.popup_notifications.retain(|n| n.id != notif.id);
                    self.popup_notifications.insert(0, notif);
                    self.ensure_popup_window()
                }
                NotifEvent::Closed(id) => {
                    self.popup_notifications.retain(|n| n.id != id);
                    if self.hovered_notif_id == Some(id) {
                        self.hovered_notif_id = None;
                    }
                    self.maybe_close_popup_window()
                }
            },
            Message::NotifDismiss(id) => {
                self.popup_notifications.retain(|n| n.id != id);
                if self.hovered_notif_id == Some(id) {
                    self.hovered_notif_id = None;
                }
                self.maybe_close_popup_window()
            }
            Message::NotifHoverEnter(id) => {
                self.hovered_notif_id = Some(id);
                Task::none()
            }
            Message::NotifHoverExit(id) => {
                if self.hovered_notif_id == Some(id) {
                    self.hovered_notif_id = None;
                }
                Task::none()
            }
            Message::NotifActivate(id) => {
                let notif = self.popup_notifications.iter().find(|n| n.id == id);
                let action_key = notif
                    .and_then(|n| n.actions.first())
                    .map_or_else(|| "default".to_string(), |(key, _)| key.clone());
                let app_name = notif.map(|n| n.app_name.clone());
                self.popup_notifications.retain(|n| n.id != id);
                if self.hovered_notif_id == Some(id) {
                    self.hovered_notif_id = None;
                }
                services::notifications::invoke_action(id, action_key);
                if let Some(name) = app_name {
                    services::hyprland::focus_window(&name);
                }
                self.maybe_close_popup_window()
            }
            Message::AudioPanelOpen(monitor) => {
                let close = self.close_all_panels();
                let height = style::audio_panel_height(self.audio.sinks.len());
                let open = self
                    .audio_panel
                    .open(style::AUDIO_PANEL_WIDTH, height, monitor);
                Task::batch([close, open])
            }
            Message::NetworkPanelOpen(monitor) => {
                let close = self.close_all_panels();
                let ap_count = self.network.access_points.len().clamp(1, 8);
                let conn_groups = connection_type_groups(&self.network.active_connections);
                let height =
                    style::network_panel_height(ap_count, &conn_groups, self.network.wifi_enabled);
                services::network::set_panel_open(true);
                let open = self
                    .network_panel
                    .open(style::NETWORK_PANEL_WIDTH, height, monitor);
                Task::batch([close, open])
            }
            Message::BatteryPanelOpen(monitor) => {
                let close = self.close_all_panels();
                let height = style::battery_panel_height(self.battery.power_profiles.is_some());
                let open = self
                    .battery_panel
                    .open(style::BATTERY_PANEL_WIDTH, height, monitor);
                Task::batch([close, open])
            }
            Message::BluetoothPanelOpen(monitor) => {
                let close = self.close_all_panels();
                let paired = self
                    .bluetooth
                    .devices
                    .iter()
                    .filter(|d| d.paired)
                    .count()
                    .clamp(1, 8);
                let nearby = self
                    .bluetooth
                    .devices
                    .iter()
                    .filter(|d| !d.paired)
                    .count()
                    .min(8);
                let height = style::bluetooth_panel_height(
                    paired,
                    nearby,
                    self.bluetooth.powered,
                    self.bluetooth.discovering,
                );
                services::bluetooth::set_panel_open(true);
                let open = self
                    .bluetooth_panel
                    .open(style::BLUETOOTH_PANEL_WIDTH, height, monitor);
                Task::batch([close, open])
            }
            Message::SysinfoPanelOpen(monitor) => {
                let close = self.close_all_panels();
                let height = style::sysinfo_panel_height();
                services::sysinfo::set_panel_open(true);
                let open = self
                    .sysinfo_panel
                    .open(style::SYSINFO_PANEL_WIDTH, height, monitor);
                Task::batch([close, open])
            }
            Message::BluetoothToggleDevice { path, connected } => {
                services::bluetooth::toggle_device_connection(&path, connected);
                Task::none()
            }
            Message::BluetoothSetPowered(powered) => {
                services::bluetooth::set_adapter_powered(powered);
                Task::none()
            }
            Message::BluetoothSetDiscovery(active) => {
                services::bluetooth::set_discovery(active);
                Task::none()
            }
            Message::BluetoothForgetDevice(path) => {
                services::bluetooth::remove_device(&path);
                Task::none()
            }
            Message::NetworkSetWifiEnabled(enabled) => {
                self.network.wifi_enabled = enabled;
                if !enabled {
                    self.network.icon_name = obayebar::style::ICON_WIFI_OFF;
                }
                services::network::set_wifi_enabled(enabled);
                Task::none()
            }
            Message::NetworkConnect(ssid) => {
                self.connecting_ssid = Some(ssid.clone());
                services::network::connect_network(ssid);
                Task::none()
            }
            Message::NetworkDisconnect => {
                self.connecting_ssid = None;
                services::network::disconnect_wifi();
                Task::none()
            }
            Message::CloseAllPanels => self.close_all_panels(),
            Message::AudioSetVolume(vol) => {
                self.audio.volume = vol;
                self.audio.icon_name = crate::services::audio::volume_icon(vol, self.audio.muted);
                services::audio::send_command(AudioCommand::Volume(vol));
                Task::none()
            }
            Message::AudioSetMute(muted) => {
                self.audio.muted = muted;
                self.audio.icon_name =
                    crate::services::audio::volume_icon(self.audio.volume, muted);
                services::audio::send_command(AudioCommand::Mute(muted));
                Task::none()
            }
            Message::AudioSetDefaultSink(id) => {
                services::audio::send_command(AudioCommand::DefaultSink { id });
                Task::none()
            }
            Message::SetPowerProfile(profile) => {
                services::battery::set_power_profile(&profile);
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

        let mut tasks = Vec::new();

        // Close bars for monitors that are no longer connected
        let removed: Vec<String> = self
            .monitors_with_bars
            .iter()
            .filter(|m| !state.monitors.contains(m))
            .cloned()
            .collect();
        for monitor in &removed {
            // Close extra bar windows for removed monitors
            let ids_to_remove: Vec<window::Id> = self
                .extra_bar_windows
                .iter()
                .filter(|(_, m)| *m == monitor)
                .map(|(id, _)| *id)
                .collect();
            for id in ids_to_remove {
                self.extra_bar_windows.remove(&id);
                tasks.push(close_window(id));
            }
            // If the initial monitor was removed, reassign to a remaining monitor
            if self.initial_monitor.as_deref() == Some(monitor) {
                self.initial_monitor = state.monitors.first().cloned();
            }
            self.ws_spring.remove(monitor);
            self.ws_cache.remove(monitor);
        }
        self.monitors_with_bars.retain(|m| !removed.contains(m));

        // Create bars for any monitors we haven't seen yet
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
        } else if self.audio_panel.is_window(id) {
            bar::audio_panel::view(&self.audio)
        } else if self.network_panel.is_window(id) {
            bar::network_panel::view(&self.network, self.connecting_ssid.as_deref())
        } else if self.battery_panel.is_window(id) {
            bar::battery_panel::view(&self.battery)
        } else if self.bluetooth_panel.is_window(id) {
            bar::bluetooth_panel::view(&self.bluetooth)
        } else if self.sysinfo_panel.is_window(id) {
            bar::sysinfo_panel::view(&self.sysinfo)
        } else {
            let monitor = self.monitor_for_bar(id);
            bar::view(self, monitor)
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let is_animating = self.ws_spring.values().any(SpringState::is_animating);

        let mut subs = vec![
            Subscription::run(services::timers::clock_stream).map(|_| Message::Tick),
            Subscription::run(services::hyprland::stream).map(Message::Hyprland),
            Subscription::run(services::battery::stream).map(Message::Battery),
            Subscription::run(services::network::stream).map(Message::Network),
            Subscription::run(services::audio::stream).map(Message::Audio),
            Subscription::run(services::tray::stream).map(Message::TrayItems),
            Subscription::run(services::bluetooth::stream).map(Message::Bluetooth),
            Subscription::run(services::sysinfo::stream).map(Message::SysInfo),
            Subscription::run(services::notifications::stream).map(Message::Notif),
        ];

        // Wake at the earliest pending popup expiry so we can retire it.
        // The subscription's identity is the instant itself: when a new popup
        // with a sooner expiry arrives, iced tears the old wake down and
        // spawns a fresh one.
        if let Some(next) = self
            .popup_notifications
            .iter()
            .filter_map(|n| n.expire_at)
            .min()
        {
            subs.push(
                Subscription::run_with(next, |at| services::timers::wake_at(*at))
                    .map(|()| Message::Tick),
            );
        }

        if is_animating {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(16)).map(|_| Message::AnimTick),
            );
        }

        Subscription::batch(subs)
    }

    fn close_all_panels(&mut self) -> Task<Message> {
        services::network::set_panel_open(false);
        services::bluetooth::set_panel_open(false);
        services::sysinfo::set_panel_open(false);
        Task::batch([
            self.audio_panel.close(),
            self.network_panel.close(),
            self.battery_panel.close(),
            self.bluetooth_panel.close(),
            self.sysinfo_panel.close(),
        ])
    }

    fn expire_popups(&mut self) -> Task<Message> {
        let now = chrono::Local::now();
        self.popup_notifications
            .retain(|n| n.expire_at.is_none_or(|exp| now < exp));
        if let Some(hovered) = self.hovered_notif_id {
            if !self.popup_notifications.iter().any(|n| n.id == hovered) {
                self.hovered_notif_id = None;
            }
        }
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

/// Count connections per type group (preserving insertion order).
fn connection_type_groups(conns: &[services::network::ActiveConnectionInfo]) -> Vec<usize> {
    let mut groups: Vec<(&str, usize)> = Vec::new();
    for ac in conns {
        if let Some(g) = groups.iter_mut().find(|(t, _)| *t == ac.conn_type) {
            g.1 += 1;
        } else {
            groups.push((&ac.conn_type, 1));
        }
    }
    groups.into_iter().map(|(_, c)| c).collect()
}

fn theme_fn(_app: &App, _id: window::Id) -> Theme {
    style::m3_theme("obayebar-dark")
}

fn close_window(id: window::Id) -> Task<Message> {
    iced_runtime::task::effect(iced_runtime::Action::Window(
        iced_runtime::window::Action::Close(id),
    ))
}
