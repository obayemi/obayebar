mod bar;
mod config;
mod notifications;
mod panel;
mod services;

use obayebar::style;

use std::cell::Cell;
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
use panel::PanelKind;
use services::audio::{AudioCommand, AudioInfo};
use services::battery::BatteryInfo;
use services::bluetooth::BluetoothInfo;
use services::gitlab::GitlabInfo;
use services::hyprland::{HyprEvent, HyprState, MonitorGeom, WindowInfo, WorkspaceInfo};
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

/// How long a Wi-Fi connection attempt may show its spinner before we give up
/// on it. `NetworkManager` authenticates asynchronously, so a bad passphrase or
/// a missing secret agent can fail without any state change we could observe —
/// this bounds the spinner so the row's connect button comes back.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Parsed CLI arguments. Kept intentionally small — extend with care.
#[derive(Debug, Default, Clone)]
struct CliArgs {
    /// `Some(true)` when `--gitlab` was passed; `None` means "use config".
    gitlab_enable: Option<bool>,
    /// `--gitlab-url <URL>`; `None` means "use env or config".
    gitlab_url: Option<String>,
}

fn print_usage() {
    println!(
        "obayebar — wayland status bar\n\
         \n\
         Usage: obayebar [OPTIONS]\n\
         \n\
         Options:\n  \
           --gitlab              Show the GitLab todos module on the bar\n  \
           --gitlab-url <URL>    Base URL of the GitLab instance (overrides config / env)\n  \
           -h, --help            Print this help\n  \
           -V, --version         Print version\n\
         \n\
         Persistent settings can also be placed in $XDG_CONFIG_HOME/obayebar/config.toml\n\
         (see [gitlab].enable / [gitlab].url).\n"
    );
}

fn parse_cli() -> CliArgs {
    let mut args = CliArgs::default();
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        let url_value = match arg.as_str() {
            "--gitlab" => {
                args.gitlab_enable = Some(true);
                continue;
            }
            "--gitlab-url" => iter.next().unwrap_or_else(|| {
                eprintln!("obayebar: --gitlab-url requires a value");
                print_usage();
                std::process::exit(2);
            }),
            s if s.starts_with("--gitlab-url=") => s["--gitlab-url=".len()..].to_string(),
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("obayebar {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            other => {
                eprintln!("obayebar: unknown argument '{other}'");
                print_usage();
                std::process::exit(2);
            }
        };
        if url_value.is_empty() {
            eprintln!("obayebar: --gitlab-url value cannot be empty");
            std::process::exit(2);
        }
        args.gitlab_url = Some(url_value);
    }
    args
}

fn main() {
    let args = parse_cli();

    let cli = config::CliOverrides {
        gitlab_enable: args.gitlab_enable,
        gitlab_url: args.gitlab_url,
    };
    config::install(&config::Config::load(), &cli);

    let logger = env_logger::Builder::from_default_env().build();
    let max_level = logger.filter();
    log::set_boxed_logger(Box::new(FatalErrorLogger { inner: logger }))
        .map(|()| log::set_max_level(max_level))
        .ok();

    // The bar has no surface with keyboard interactivity, so the smithay-
    // clipboard worker would never see a Ctrl+V anyway. Skip spawning it.
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
    /// Map of bar window ID -> monitor name (for extra monitors only).
    /// This — together with `initial_monitor` — is the canonical record of
    /// which monitors have a bar. Any other view must derive from these two.
    extra_bar_windows: HashMap<window::Id, String>,
    /// The monitor that the initial (settings-created) window is on
    initial_monitor: Option<String>,
    /// Window id of the settings-created initial bar. We don't generate this
    /// id ourselves, so we capture it lazily on the first `view()` call for
    /// an unknown bar surface — only an exact match should be treated as the
    /// initial bar in close-event handling. Without this, popup/panel close
    /// events (which clear their tracking before the close fires) get
    /// misclassified as the initial bar and trigger spurious bar respawns.
    initial_bar_id: Cell<Option<window::Id>>,
    /// Per-monitor workspace indicator spring animation
    ws_spring: HashMap<String, SpringState>,
    /// Per-monitor workspace canvas cache (cleared on data change)
    pub ws_cache: HashMap<String, canvas::Cache>,
    /// Fallback cache used before monitor-specific caches are created
    pub ws_cache_fallback: canvas::Cache,
    /// Vector font for canvas text rendering
    pub vector_font: Option<ab_glyph::FontArc>,

    notif_popup_id: Option<window::Id>,
    /// One entry per `PanelKind`. Lazily populated on first open via
    /// `Panel::default`; the only invariant is that at most one panel is open
    /// at a time (enforced by `close_all_panels` before each `open`).
    panels: HashMap<PanelKind, panel::Panel>,
    pub gitlab_enabled: bool,
    pub gitlab: GitlabInfo,
    /// Working buffer for the token input field in the GitLab popup. Persists
    /// across panel close/reopen so a stray mouse-exit doesn't lose typing.
    pub gitlab_token_input: String,

    pub workspaces: Vec<WorkspaceInfo>,
    /// Per-monitor active workspace: `monitor_name` -> `active_workspace_id`
    pub active_workspaces: HashMap<String, i32>,
    /// Physical geometry of each connected monitor, keyed by name.
    pub monitor_geoms: HashMap<String, MonitorGeom>,
    /// Name of the Hyprland-focused monitor, used as the reference output for
    /// overlays that need a screen-relative size (notification popup, etc.).
    pub focused_monitor: Option<String>,
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
    Gitlab(GitlabInfo),
    GitlabOpenUrl(String),
    GitlabOpenTokenFile,
    GitlabReloadToken,
    GitlabTokenInputChanged(String),
    GitlabTokenInputPaste,
    GitlabTokenInputPasted(Result<String, String>),
    GitlabTokenSubmit,
    GitlabTokenSaved(Result<(), String>),
    GitlabForgetToken,
    GitlabTokenForgotten(Result<(), String>),
    TrayItems(Vec<TrayItemInfo>),
    TrayClick(String),
    Notif(NotifEvent),
    NotifDismiss(u32),
    NotifActivate(u32),
    NotifHoverEnter(u32),
    NotifHoverExit(u32),
    PanelOpen(PanelKind, Option<String>),
    Bluetooth(BluetoothInfo),
    BluetoothToggleDevice {
        path: String,
        connected: bool,
    },
    BluetoothSetPowered(bool),
    BluetoothSetDiscovery(bool),
    BluetoothForgetDevice(String),
    NetworkSetWifiEnabled(bool),
    NetworkConnect(String),
    /// Outcome of the `NetworkManager` connect request itself.
    NetworkConnectDone(Result<(), String>),
    /// `CONNECT_TIMEOUT` elapsed for this SSID; clears a spinner that no
    /// `NetworkManager` state change would ever clear.
    NetworkConnectTimedOut(String),
    NetworkDisconnect,
    CloseAllPanels,
    /// Absolute target, from the audio panel's slider.
    AudioSetVolume(f32),
    /// Relative change, from scrolling the bar's volume icon. Relative because
    /// that handler lives inside a `lazy` subtree and must not capture state.
    AudioNudgeVolume(f32),
    AudioSetMute(bool),
    AudioSetDefaultSink(u32),
    AudioOpenPavucontrol,
    SetPowerProfile(String),
    WindowClosed(window::Id),
}

impl App {
    fn new() -> (Self, Task<Message>) {
        (
            Self {
                extra_bar_windows: HashMap::new(),
                initial_monitor: None,
                initial_bar_id: Cell::new(None),
                ws_spring: HashMap::new(),
                ws_cache: HashMap::new(),
                ws_cache_fallback: canvas::Cache::default(),
                vector_font: style::load_vector_font(),
                notif_popup_id: None,
                panels: HashMap::new(),
                gitlab_enabled: config::resolved().gitlab_enable(),
                gitlab: GitlabInfo::default(),
                gitlab_token_input: String::new(),
                workspaces: Vec::new(),
                active_workspaces: HashMap::new(),
                monitor_geoms: HashMap::new(),
                focused_monitor: None,
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

    /// Get the monitor name for a bar window ID. Returns `None` if `id` is
    /// not a tracked bar surface.
    fn monitor_for_bar(&self, id: window::Id) -> Option<&str> {
        if let Some(monitor) = self.extra_bar_windows.get(&id) {
            return Some(monitor);
        }
        if self.initial_bar_id.get() == Some(id) {
            return self.initial_monitor.as_deref();
        }
        // First-render fallback: `view()` captures the initial bar's id on
        // its first call; until then, an unknown id can only be that bar.
        if self.initial_bar_id.get().is_none() {
            return self.initial_monitor.as_deref();
        }
        None
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
                services::notifications::emit_closed(
                    id,
                    services::notifications::close_reason::DISMISSED,
                );
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
            Message::PanelOpen(kind, monitor) => self.open_panel(kind, monitor),
            Message::Gitlab(info) => {
                if self.gitlab != info {
                    self.gitlab = info;
                }
                Task::none()
            }
            Message::GitlabOpenUrl(url) => {
                if !url.is_empty() {
                    services::gitlab::open_in_browser(url);
                }
                self.close_all_panels()
            }
            Message::GitlabOpenTokenFile => {
                services::gitlab::open_token_file();
                Task::none()
            }
            Message::GitlabReloadToken => {
                services::gitlab::request_refresh();
                Task::none()
            }
            Message::GitlabTokenInputChanged(value) => {
                self.gitlab_token_input = value;
                Task::none()
            }
            Message::GitlabTokenInputPaste => Task::perform(
                services::gitlab::read_clipboard(),
                Message::GitlabTokenInputPasted,
            ),
            Message::GitlabTokenInputPasted(Ok(text)) => {
                self.gitlab_token_input = text.trim().to_string();
                Task::none()
            }
            Message::GitlabTokenInputPasted(Err(msg))
            | Message::GitlabTokenSaved(Err(msg))
            | Message::GitlabTokenForgotten(Err(msg)) => {
                self.gitlab.error = Some(msg);
                Task::none()
            }
            Message::GitlabTokenSubmit => {
                let token = std::mem::take(&mut self.gitlab_token_input);
                Task::perform(
                    services::gitlab::save_token(token),
                    Message::GitlabTokenSaved,
                )
            }
            Message::GitlabTokenSaved(Ok(())) | Message::GitlabTokenForgotten(Ok(())) => {
                services::gitlab::request_refresh();
                self.close_all_panels()
            }
            Message::GitlabForgetToken => Task::perform(
                services::gitlab::forget_token(),
                Message::GitlabTokenForgotten,
            ),
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
                // Two independent ways out of the spinner, because neither
                // covers the other: the `Result` catches a request that fails
                // outright, and the deadline catches an authentication failure
                // that never changes NetworkManager's state (so no
                // `NetworkInfo` update ever arrives to clear it). Without both,
                // the row keeps spinning and the panel hides its connect
                // button, leaving that network unretryable.
                let deadline_ssid = ssid.clone();
                Task::batch([
                    Task::perform(
                        services::network::connect_network(ssid),
                        Message::NetworkConnectDone,
                    ),
                    Task::perform(
                        async move {
                            tokio::time::sleep(CONNECT_TIMEOUT).await;
                            deadline_ssid
                        },
                        Message::NetworkConnectTimedOut,
                    ),
                ])
            }
            Message::NetworkConnectDone(Err(reason)) => {
                log::warn!("network: {reason}");
                self.connecting_ssid = None;
                Task::none()
            }
            Message::NetworkConnectTimedOut(ssid) => {
                // Only clear if this is still the attempt we started: the user
                // may have moved on to a different network in the meantime.
                if self.connecting_ssid.as_deref() == Some(ssid.as_str()) {
                    log::warn!("network: connecting to {ssid} timed out");
                    self.connecting_ssid = None;
                }
                Task::none()
            }
            Message::NetworkDisconnect => {
                self.connecting_ssid = None;
                services::network::disconnect_wifi();
                Task::none()
            }
            Message::CloseAllPanels => self.close_all_panels(),
            Message::AudioSetVolume(vol) => self.set_volume(vol),
            Message::AudioNudgeVolume(delta) => {
                // The bar's scroll handler is cached by `lazy`, so it sends a
                // relative delta and the base volume is resolved here.
                self.set_volume(self.audio.volume + delta)
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
            Message::WindowClosed(id) => self.handle_window_closed(id),
            _ => Task::none(),
        }
    }

    /// Apply a full Hyprland state update. Creates bar windows for new monitors.
    fn apply_hypr_state(&mut self, state: HyprState) -> Task<Message> {
        self.workspaces = state.workspaces;
        self.active_window = state.active_window;
        self.monitor_geoms = state.monitor_geoms;
        self.focused_monitor = Some(state.focused_monitor.clone());

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
        // Assign it on first state update so reconciliation can match the
        // bar against an expected monitor instead of treating it as missing.
        if self.initial_monitor.is_none() && self.extra_bar_windows.is_empty() {
            self.initial_monitor = Some(state.focused_monitor);
        }

        let mut tasks = vec![self.reconcile_bars()];

        // Re-fit notification popup: focused monitor or its geometry may have
        // changed, which affects the 2/5-of-screen cap.
        if self.notif_popup_id.is_some() && !self.popup_notifications.is_empty() {
            tasks.push(self.ensure_popup_window());
        }

        Task::batch(tasks)
    }

    /// Spawn a layer-shell bar pinned to `monitor` and register it in
    /// `extra_bar_windows`. Internal primitive — call sites must go through
    /// `reconcile_bars` so the per-monitor invariants stay enforced.
    fn spawn_bar_for(&mut self, monitor: String) -> Task<Message> {
        let id = window::Id::unique();
        self.extra_bar_windows.insert(id, monitor.clone());
        Task::done(Message::NewLayerShell {
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
        })
    }

    /// Drop per-monitor workspace state — the surface is gone (compositor
    /// closed it) so its workspace cache/spring should not linger either.
    fn drop_bar_state(&mut self, monitor: &str) {
        self.ws_spring.remove(monitor);
        self.ws_cache.remove(monitor);
    }

    /// Enforce the bar-distribution invariants over the set of currently
    /// connected monitors (those with known geometry):
    ///
    /// 1. Every connected monitor has a bar pinned to it.
    /// 2. No bar is left over for a disconnected monitor.
    /// 3. No two bars share the same monitor.
    ///
    /// This is the single source of truth for redistribution. Every code
    /// path that touches monitor presence or bar window lifetimes must
    /// finish by calling this so the model stays coherent regardless of how
    /// it got there. The function is idempotent — calling it twice in a
    /// row is a no-op.
    fn reconcile_bars(&mut self) -> Task<Message> {
        let expected: std::collections::HashSet<String> =
            self.monitor_geoms.keys().cloned().collect();
        let plan = plan_bar_reconcile(
            &expected,
            self.initial_monitor.as_deref(),
            &self.extra_bar_windows,
        );

        let mut tasks = Vec::new();

        if plan.drop_initial {
            self.initial_monitor = None;
            self.initial_bar_id.set(None);
        }
        for id in &plan.close_extras {
            self.extra_bar_windows.remove(id);
            tasks.push(close_window(*id));
        }
        for monitor in &plan.drop_state_for {
            self.drop_bar_state(monitor);
        }
        for monitor in plan.spawn_monitors {
            tasks.push(self.spawn_bar_for(monitor));
        }

        #[cfg(debug_assertions)]
        debug_assert!(self.bar_invariants_hold());

        Task::batch(tasks)
    }

    /// Debug-only check that the three bar-distribution invariants hold.
    /// Used after `reconcile_bars` to catch reconciler bugs at the source.
    #[cfg(debug_assertions)]
    #[allow(clippy::arithmetic_side_effects)]
    fn bar_invariants_hold(&self) -> bool {
        use std::collections::HashMap;
        let mut counts: HashMap<&str, u32> = HashMap::new();
        if let Some(m) = &self.initial_monitor {
            *counts.entry(m).or_insert(0) += 1;
        }
        for m in self.extra_bar_windows.values() {
            *counts.entry(m).or_insert(0) += 1;
        }
        for (monitor, count) in &counts {
            if !self.monitor_geoms.contains_key(*monitor) {
                log::error!("bar invariant: stale bar tracked for {monitor}");
                return false;
            }
            if *count > 1 {
                log::error!("bar invariant: {count} bars on monitor {monitor}");
                return false;
            }
        }
        for monitor in self.monitor_geoms.keys() {
            if !counts.contains_key(monitor.as_str()) {
                log::error!("bar invariant: monitor {monitor} has no bar");
                return false;
            }
        }
        true
    }

    /// Handle a window closed by the compositor. Layer surfaces are torn
    /// down when their `wl_output` disappears (monitor disconnect, screen
    /// sleep, etc.); without this, our tracking would drift and bars get
    /// stranded on a single output when others come back.
    fn handle_window_closed(&mut self, id: window::Id) -> Task<Message> {
        if self.notif_popup_id == Some(id) {
            self.notif_popup_id = None;
            return self.ensure_popup_window();
        }
        if let Some(kind) = self.forget_panel_window(id) {
            if let Some(setter) = kind.signal_setter() {
                setter(false);
            }
            return Task::none();
        }

        // Bar window: drop our tracking for the closed surface, then let
        // `reconcile_bars` decide whether to respawn (monitor still around)
        // or do nothing (monitor gone). Unknown ids are popup/panel
        // surfaces whose tracking we cleared before initiating close —
        // treat them as no-ops so notification dismissals don't spawn bars.
        let closed_a_bar = if let Some(monitor) = self.extra_bar_windows.remove(&id) {
            if !self.extra_bar_windows.values().any(|m| m == &monitor)
                && self.initial_monitor.as_deref() != Some(&monitor)
            {
                self.drop_bar_state(&monitor);
            }
            true
        } else if self.initial_bar_id.get() == Some(id) {
            self.initial_bar_id.set(None);
            if let Some(monitor) = self.initial_monitor.take() {
                if !self.extra_bar_windows.values().any(|m| m == &monitor) {
                    self.drop_bar_state(&monitor);
                }
            }
            true
        } else {
            false
        };

        if closed_a_bar {
            self.reconcile_bars()
        } else {
            Task::none()
        }
    }

    fn view(&self, id: window::Id) -> Element<'_, Message> {
        if Some(id) == self.notif_popup_id {
            return notifications::popup_view(self);
        }
        if let Some(kind) = self
            .panels
            .iter()
            .find_map(|(k, p)| p.is_window(id).then_some(*k))
        {
            return self.view_panel(kind);
        }
        // Capture the settings-created bar's id the first time we render
        // it so close events can be matched exactly rather than guessed.
        if !self.extra_bar_windows.contains_key(&id) && self.initial_bar_id.get().is_none() {
            self.initial_bar_id.set(Some(id));
        }
        let monitor = self.monitor_for_bar(id);
        bar::view(self, monitor)
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
            iced::window::close_events().map(Message::WindowClosed),
        ];

        if self.gitlab_enabled {
            subs.push(Subscription::run(services::gitlab::stream).map(Message::Gitlab));
        }

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

    /// Apply `volume` optimistically and forward it to `PipeWire`. The optimistic
    /// update keeps the slider and icon responsive; the service's next
    /// `AudioInfo` corrects it if the write did not land.
    fn set_volume(&mut self, volume: f32) -> Task<Message> {
        let volume = volume.clamp(0.0, 1.0);
        self.audio.volume = volume;
        self.audio.icon_name = services::audio::volume_icon(volume, self.audio.muted);
        services::audio::send_command(AudioCommand::Volume(volume));
        Task::none()
    }

    fn close_all_panels(&mut self) -> Task<Message> {
        for kind in PanelKind::ALL {
            if let Some(setter) = kind.signal_setter() {
                setter(false);
            }
        }
        let tasks: Vec<_> = self.panels.values_mut().map(panel::Panel::close).collect();
        Task::batch(tasks)
    }

    /// Compute the layer-shell surface size for `kind` from current state.
    /// Width is fixed; height adapts to dynamic content (sink count, AP count,
    /// paired/nearby split, …).
    fn panel_dimensions(&self, kind: PanelKind) -> (u32, u32) {
        let height = match kind {
            PanelKind::Audio => style::audio_panel_height(self.audio.sinks.len()),
            PanelKind::Network => {
                let ap_count = self.network.access_points.len().clamp(1, 8);
                let conn_groups = connection_type_groups(&self.network.active_connections);
                style::network_panel_height(ap_count, &conn_groups, self.network.wifi_enabled)
            }
            PanelKind::Battery => {
                style::battery_panel_height(self.battery.power_profiles.is_some())
            }
            PanelKind::Bluetooth => {
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
                style::bluetooth_panel_height(
                    paired,
                    nearby,
                    self.bluetooth.powered,
                    self.bluetooth.discovering,
                )
            }
            PanelKind::Sysinfo => style::sysinfo_panel_height(),
            PanelKind::Gitlab => style::GITLAB_PANEL_HEIGHT,
        };
        (kind.width(), height)
    }

    /// Render the body of `kind`'s popup. The dispatch table for `view()`.
    fn view_panel(&self, kind: PanelKind) -> Element<'_, Message> {
        match kind {
            PanelKind::Audio => bar::audio_panel::view(&self.audio),
            PanelKind::Network => {
                bar::network_panel::view(&self.network, self.connecting_ssid.as_deref())
            }
            PanelKind::Battery => bar::battery_panel::view(&self.battery),
            PanelKind::Bluetooth => bar::bluetooth_panel::view(&self.bluetooth),
            PanelKind::Sysinfo => bar::sysinfo_panel::view(&self.sysinfo),
            PanelKind::Gitlab => bar::gitlab_panel::view(&self.gitlab, &self.gitlab_token_input),
        }
    }

    /// Open `kind`'s popup, replacing whichever panel is currently shown.
    fn open_panel(&mut self, kind: PanelKind, monitor: Option<String>) -> Task<Message> {
        let close = self.close_all_panels();
        let (width, height) = self.panel_dimensions(kind);
        if let Some(setter) = kind.signal_setter() {
            setter(true);
        }
        let open = self
            .panels
            .entry(kind)
            .or_default()
            .open(width, height, monitor);
        Task::batch([close, open])
    }

    /// If `id` matches an open panel surface, drop the tracking and return
    /// which kind it was. Returns `None` for non-panel windows.
    fn forget_panel_window(&mut self, id: window::Id) -> Option<PanelKind> {
        self.panels
            .iter_mut()
            .find_map(|(k, p)| p.forget_if(id).then_some(*k))
    }

    fn expire_popups(&mut self) -> Task<Message> {
        let now = chrono::Local::now();
        // Collect the ids first: `Vec::retain` hands the closure nothing we can
        // report on, and a client waiting on `NotificationClosed` needs the
        // expiry signal as much as the dismissal one.
        let expired: Vec<u32> = self
            .popup_notifications
            .iter()
            .filter(|n| n.expire_at.is_some_and(|exp| now >= exp))
            .map(|n| n.id)
            .collect();
        self.popup_notifications
            .retain(|n| n.expire_at.is_none_or(|exp| now < exp));
        for id in expired {
            services::notifications::emit_closed(
                id,
                services::notifications::close_reason::EXPIRED,
            );
        }
        if let Some(hovered) = self.hovered_notif_id {
            if !self.popup_notifications.iter().any(|n| n.id == hovered) {
                self.hovered_notif_id = None;
            }
        }
        self.maybe_close_popup_window()
    }

    /// Maximum popup height in logical pixels: 2/5 of the focused monitor's
    /// logical height, or a conservative 1080p-based fallback.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::as_conversions
    )]
    fn popup_max_height(&self) -> u32 {
        const FALLBACK_LOGICAL_H: f32 = 1080.0;
        let num = f32::from(u16::try_from(style::NOTIF_POPUP_MAX_FRACTION_NUM).unwrap_or(2));
        let den = f32::from(u16::try_from(style::NOTIF_POPUP_MAX_FRACTION_DEN).unwrap_or(5));

        let geom = self
            .focused_monitor
            .as_deref()
            .and_then(|name| self.monitor_geoms.get(name));

        let logical_h = geom.map_or(FALLBACK_LOGICAL_H, |g| {
            let scale = if g.scale > 0.0 { g.scale } else { 1.0 };
            // Transforms 1/3/5/7 rotate by 90° or 270°, swapping width/height.
            let raw = match g.transform {
                1 | 3 | 5 | 7 => g.width,
                _ => g.height,
            };
            raw as f32 / scale
        });

        (logical_h * num / den) as u32
    }

    /// Decide how many popup cards fit and how many spill into an overflow
    /// summary entry, using the focused monitor's screen cap.
    fn popup_fit(&self) -> (usize, usize) {
        style::notif_popup_fit(self.popup_notifications.len(), self.popup_max_height())
    }

    fn ensure_popup_window(&mut self) -> Task<Message> {
        if self.popup_notifications.is_empty() {
            return Task::none();
        }
        let (visible, overflow) = self.popup_fit();
        let height = style::notif_popup_height(visible, overflow);
        if let Some(id) = self.notif_popup_id {
            // Resize existing window to fit current notification layout
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

/// Decision output from `plan_bar_reconcile`: what changes the caller must
/// apply for the three bar-distribution invariants to hold.
#[derive(Debug, Default, PartialEq, Eq)]
struct BarReconcilePlan {
    /// Initial bar tracking should be cleared (its monitor disappeared).
    drop_initial: bool,
    /// Extra bar window ids to close (sorted by id for determinism).
    close_extras: Vec<window::Id>,
    /// Monitors that lose their last bar in this plan and whose per-monitor
    /// state (workspace cache/spring) should be dropped (sorted).
    drop_state_for: Vec<String>,
    /// Monitors that need a fresh bar spawned (sorted).
    spawn_monitors: Vec<String>,
}

/// Compute the redistribution plan for a given bar tracking state. Pure —
/// returns the operations needed to restore the three invariants without
/// touching any iced types or running side effects.
fn plan_bar_reconcile(
    expected: &std::collections::HashSet<String>,
    initial_monitor: Option<&str>,
    extras: &HashMap<window::Id, String>,
) -> BarReconcilePlan {
    use std::collections::HashSet;

    let drop_initial = initial_monitor.is_some_and(|m| !expected.contains(m));
    let live_initial = if drop_initial { None } else { initial_monitor };

    let mut sorted_extras: Vec<(window::Id, &str)> =
        extras.iter().map(|(id, m)| (*id, m.as_str())).collect();
    sorted_extras.sort_by_key(|(id, _)| *id);

    // First pass: walk extras in id order, keeping the first bar seen per
    // monitor and marking the rest (or any on a stale monitor) for close.
    let mut have: HashSet<&str> = live_initial.into_iter().collect();
    let mut close_extras: Vec<window::Id> = Vec::new();
    for (id, monitor) in &sorted_extras {
        if !expected.contains(*monitor) || !have.insert(monitor) {
            close_extras.push(*id);
        }
    }

    // A monitor loses its last bar when it had one before the plan but has
    // none after. `have` is the post-plan set; the pre-plan set is `extras`
    // values plus the original `initial_monitor`.
    let pre_have: HashSet<&str> = extras
        .values()
        .map(String::as_str)
        .chain(initial_monitor)
        .collect();
    let mut drop_state_for: Vec<String> = pre_have
        .difference(&have)
        .map(|s| (*s).to_string())
        .collect();
    drop_state_for.sort();

    let mut spawn_monitors: Vec<String> = expected
        .iter()
        .filter(|m| !have.contains(m.as_str()))
        .cloned()
        .collect();
    spawn_monitors.sort();

    BarReconcilePlan {
        drop_initial,
        close_extras,
        drop_state_for,
        spawn_monitors,
    }
}

#[cfg(test)]
mod reconcile_tests {
    use super::{plan_bar_reconcile, BarReconcilePlan};
    use iced::window;
    use std::collections::{HashMap, HashSet};

    fn expected<const N: usize>(monitors: [&str; N]) -> HashSet<String> {
        monitors.iter().map(|s| (*s).to_string()).collect()
    }

    fn extras<const N: usize>(entries: [(window::Id, &str); N]) -> HashMap<window::Id, String> {
        entries
            .iter()
            .map(|(id, m)| (*id, (*m).to_string()))
            .collect()
    }

    fn id(n: u64) -> window::Id {
        // window::Id::unique() is process-global; tests need deterministic ids.
        // The crate's Id wraps a u64, but its only public ctor is `unique()`.
        // Generate fresh ones in a loop and use them in seen-order.
        let _ = n;
        window::Id::unique()
    }

    #[test]
    fn empty_state_with_one_monitor_spawns_one_bar() {
        let plan = plan_bar_reconcile(&expected(["DP-1"]), None, &HashMap::new());
        assert!(!plan.drop_initial);
        assert!(plan.close_extras.is_empty());
        assert_eq!(plan.spawn_monitors, vec!["DP-1".to_string()]);
        assert!(plan.drop_state_for.is_empty());
    }

    #[test]
    fn initial_only_no_extras_is_idempotent() {
        let plan = plan_bar_reconcile(&expected(["DP-1"]), Some("DP-1"), &HashMap::new());
        assert_eq!(plan, BarReconcilePlan::default());
    }

    #[test]
    fn missing_monitor_gets_a_bar_initial_kept() {
        let plan = plan_bar_reconcile(
            &expected(["DP-1", "HDMI-A-1"]),
            Some("DP-1"),
            &HashMap::new(),
        );
        assert_eq!(plan.spawn_monitors, vec!["HDMI-A-1".to_string()]);
        assert!(!plan.drop_initial);
        assert!(plan.close_extras.is_empty());
    }

    #[test]
    fn stale_extra_is_closed_and_state_dropped() {
        let a = id(1);
        let plan = plan_bar_reconcile(
            &expected(["DP-1"]),
            Some("DP-1"),
            &extras([(a, "HDMI-A-1")]),
        );
        assert_eq!(plan.close_extras, vec![a]);
        assert_eq!(plan.drop_state_for, vec!["HDMI-A-1".to_string()]);
        assert!(plan.spawn_monitors.is_empty());
    }

    #[test]
    fn duplicate_extras_keep_first_close_rest() {
        let mut ids = [id(1), id(2), id(3)];
        ids.sort();
        let plan = plan_bar_reconcile(
            &expected(["HDMI-A-1"]),
            None,
            &extras([
                (ids[0], "HDMI-A-1"),
                (ids[1], "HDMI-A-1"),
                (ids[2], "HDMI-A-1"),
            ]),
        );
        assert_eq!(plan.close_extras, vec![ids[1], ids[2]]);
        // Monitor still has one bar (ids[0]), so no state drop.
        assert!(plan.drop_state_for.is_empty());
        assert!(plan.spawn_monitors.is_empty());
    }

    #[test]
    fn extra_duplicating_initial_is_closed() {
        let a = id(1);
        let plan = plan_bar_reconcile(&expected(["DP-1"]), Some("DP-1"), &extras([(a, "DP-1")]));
        assert_eq!(plan.close_extras, vec![a]);
        assert!(plan.drop_state_for.is_empty());
        assert!(plan.spawn_monitors.is_empty());
        assert!(!plan.drop_initial);
    }

    #[test]
    fn initial_monitor_disappearing_drops_initial_and_keeps_remaining() {
        let a = id(1);
        let plan = plan_bar_reconcile(
            &expected(["HDMI-A-1"]),
            Some("DP-1"),
            &extras([(a, "HDMI-A-1")]),
        );
        assert!(plan.drop_initial);
        assert!(plan.close_extras.is_empty());
        assert_eq!(plan.drop_state_for, vec!["DP-1".to_string()]);
        assert!(plan.spawn_monitors.is_empty());
    }

    #[test]
    fn no_monitors_means_close_everything() {
        let a = id(1);
        let b = id(2);
        let plan = plan_bar_reconcile(
            &HashSet::new(),
            Some("DP-1"),
            &extras([(a, "DP-1"), (b, "HDMI-A-1")]),
        );
        assert!(plan.drop_initial);
        let mut closed = plan.close_extras.clone();
        closed.sort();
        let mut want = vec![a, b];
        want.sort();
        assert_eq!(closed, want);
        assert_eq!(
            plan.drop_state_for,
            vec!["DP-1".to_string(), "HDMI-A-1".to_string()]
        );
        assert!(plan.spawn_monitors.is_empty());
    }

    #[test]
    fn duplicate_keeps_initial_and_closes_extra_even_if_extra_id_lower() {
        // Initial bar is always preferred over an extra on the same monitor
        // (we can't close the initial by id from this side), regardless of
        // id ordering.
        let a = id(1);
        let plan = plan_bar_reconcile(&expected(["DP-1"]), Some("DP-1"), &extras([(a, "DP-1")]));
        assert_eq!(plan.close_extras, vec![a]);
    }
}
