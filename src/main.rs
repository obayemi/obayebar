mod bar;
mod config;
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
use iced_layershell::settings::{LayerShellSettings, Settings, StartMode};
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

/// Prefix for every bar's layer-shell namespace, followed by its generation.
///
/// User-visible: a Hyprland `layerrule` that matched the exact namespace
/// `obayebar` must now match a prefix, e.g. `layerrule = blur, ^obayebar`.
const BAR_NAMESPACE_PREFIX: &str = "obayebar-bar-";

/// Namespace for the notification popup surface, so `j/layers` can tell it
/// apart from a bar. Every surface previously shared the app-wide `obayebar`
/// namespace, which also meant a `layerrule` could not target one and not the
/// others.
const POPUP_NAMESPACE: &str = "obayebar-notifications";

/// Base delay before checking whether a bar landed where we asked. Long enough
/// for the compositor to map a surface we just requested.
const VERIFY_DELAY: std::time::Duration = std::time::Duration::from_millis(250);

/// Ceiling for the verification backoff, so a compositor that persistently
/// refuses to place a surface is retried slowly rather than in a hot loop.
const MAX_VERIFY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);

/// How many verification passes a freshly spawned bar gets to show up in
/// `j/layers` before we give up on it and spawn a replacement. At
/// `VERIFY_DELAY` per pass this is roughly a second of grace, which covers
/// normal mapping latency without stranding a monitor for long.
const VERIFY_GRACE_PASSES: u32 = 4;

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

    // Background start mode: the app creates *every* bar surface itself, via
    // NewLayerShell in `reconcile_bars`.
    //
    // The alternative — letting `Settings` create an initial window — is what
    // made the reported duplicate/missing bars unfixable. That surface is
    // built with `becreated == false`, so layershellev's `remove_shell`
    // refuses to close it, and without a binding its `Closed` delivery is
    // unreliable; it also lands on whichever output the compositor picks, with
    // no way to find out which. So it could neither be placed, verified, nor
    // closed — only guessed at. Owning every surface removes that whole class.
    //
    // Background mode also stops layershellev calling `signal.stop()` when the
    // last unit dies, which previously turned a transient "no monitors" state
    // into process death.
    let result = iced_layershell::daemon(App::new, App::namespace, App::update, App::view)
        .settings(Settings {
            layer_settings: LayerShellSettings {
                anchor: Anchor::Left | Anchor::Top | Anchor::Bottom,
                layer: Layer::Top,
                exclusive_zone: i32::try_from(style::BAR_WIDTH).unwrap_or(54),
                size: Some((style::BAR_WIDTH, 0)),
                keyboard_interactivity: KeyboardInteractivity::None,
                start_mode: StartMode::Background,
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
    /// Every bar surface we have asked the compositor for, keyed by window id.
    ///
    /// This is bookkeeping, not truth: a record says which monitor we *asked*
    /// for, and `BarRecord::verified` says whether the compositor was ever
    /// observed agreeing. `reconcile_bars` is the only thing allowed to mutate
    /// it, and it does so from a `j/layers` observation.
    bars: HashMap<window::Id, BarRecord>,
    /// Monotonic counter making each bar's layer-shell namespace unique, so a
    /// `j/layers` observation can be matched back to one specific surface.
    bar_generation: u64,
    /// Delay before the next verification pass. Grows when a spawn fails to
    /// appear so a compositor that refuses us is not hammered; reset whenever
    /// the monitor set changes or a bar verifies.
    verify_backoff: std::time::Duration,
    /// Per-monitor workspace indicator spring animation
    ws_spring: HashMap<String, SpringState>,
    /// Per-monitor workspace canvas cache (cleared on data change)
    pub ws_cache: HashMap<String, canvas::Cache>,
    /// Fallback cache used before monitor-specific caches are created
    pub ws_cache_fallback: canvas::Cache,
    /// Vector font for canvas text rendering
    pub vector_font: Option<ab_glyph::FontArc>,

    notif_popup_id: Option<window::Id>,
    /// The monitor the popup surface was created on. Needed because its size
    /// cap depends on that monitor and because moving it requires recreating
    /// the surface rather than resizing it.
    notif_popup_monitor: Option<String>,
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
    /// A `j/layers` observation, or `None` if the query failed. Drives the
    /// whole bar reconcile loop.
    LayersObserved(Option<services::hyprland::LayerMap>),
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
                bars: HashMap::new(),
                bar_generation: 0,
                verify_backoff: VERIFY_DELAY,
                ws_spring: HashMap::new(),
                ws_cache: HashMap::new(),
                ws_cache_fallback: canvas::Cache::default(),
                vector_font: style::load_vector_font(),
                notif_popup_id: None,
                notif_popup_monitor: None,
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
    ///
    /// An exact lookup, with no fallback. The old version returned
    /// `initial_monitor` for *any* unknown id while `initial_bar_id` was
    /// unset, which meant a panel or popup surface could be rendered as a bar
    /// for the wrong monitor.
    fn monitor_for_bar(&self, id: window::Id) -> Option<&str> {
        self.bars.get(&id).map(|record| record.monitor.as_str())
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
                if !self.audio.available {
                    log::warn!("audio: ignoring mute change, PipeWire is unavailable");
                    return Task::none();
                }
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
            Message::LayersObserved(observed) => self.reconcile_bars(observed.as_ref()),
            Message::WindowClosed(id) => self.handle_window_closed(id),
            _ => Task::none(),
        }
    }

    /// Apply a full Hyprland state update, then kick off bar verification.
    fn apply_hypr_state(&mut self, state: HyprState) -> Task<Message> {
        // Snapshot the monitor *set* before overwriting it. Panels and the
        // notification popup are pinned to a specific output, so a topology
        // change has to invalidate them; comparing key sets (not `keys()`,
        // whose HashMap order is nondeterministic) is what detects that.
        let previous_monitors: std::collections::HashSet<String> =
            self.monitor_geoms.keys().cloned().collect();

        self.workspaces = state.workspaces;
        self.active_window = state.active_window;
        self.monitor_geoms = state.monitor_geoms;
        self.focused_monitor = Some(state.focused_monitor.clone());

        let monitors_changed = previous_monitors
            != self
                .monitor_geoms
                .keys()
                .cloned()
                .collect::<std::collections::HashSet<String>>();

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

        let mut tasks = Vec::new();

        if monitors_changed {
            // A fresh topology is the one moment worth retrying eagerly, so
            // drop any accumulated backoff.
            self.verify_backoff = VERIFY_DELAY;
            // Panels and the popup are pinned to an output that may have just
            // gone away. Close rather than try to correlate: `Panel` does not
            // record its monitor, and the pointer is not necessarily anywhere
            // near a panel we would otherwise leave stranded as a
            // click-swallowing overlay on a dead screen.
            //
            // Deliberately keyed on the monitor *set*, not `focused_monitor`:
            // focus-follows-mouse churns that on ordinary pointer movement and
            // would yank panels out from under the user mid-interaction.
            tasks.push(self.close_all_panels());
            tasks.push(self.invalidate_popup());
        }

        tasks.push(self.verify_bars_soon());

        // Re-fit notification popup: focused monitor or its geometry may have
        // changed, which affects the 2/5-of-screen cap.
        if self.notif_popup_id.is_some() && !self.popup_notifications.is_empty() {
            tasks.push(self.ensure_popup_window());
        }

        Task::batch(tasks)
    }

    /// Ask the compositor where our bars actually are, after `verify_backoff`.
    ///
    /// The delay exists because a surface is not mapped the instant
    /// `NewLayerShell` is queued; verifying immediately would see nothing and
    /// conclude the spawn failed.
    fn verify_bars_soon(&self) -> Task<Message> {
        let delay = self.verify_backoff;
        Task::perform(
            async move {
                tokio::time::sleep(delay).await;
                services::hyprland::fetch_layer_namespaces().await
            },
            Message::LayersObserved,
        )
    }

    /// Spawn one layer-shell bar aimed at `monitor`, under a unique namespace.
    ///
    /// The namespace is the whole point: `OutputOption::OutputName` is a
    /// request, not a guarantee — on a name-cache miss layershellev creates the
    /// surface with no output and the compositor puts it on the focused
    /// monitor, reporting nothing back. A per-surface namespace is what lets
    /// the next `j/layers` observation say which monitor this specific surface
    /// landed on. All bars previously shared the app-wide `obayebar`
    /// namespace, which made them indistinguishable and verification
    /// impossible.
    ///
    /// Private on purpose: `reconcile_bars` is the only legitimate caller.
    fn spawn_bar_for(&mut self, monitor: String) -> Task<Message> {
        self.bar_generation = self.bar_generation.wrapping_add(1);
        let namespace = format!("{BAR_NAMESPACE_PREFIX}{}", self.bar_generation);
        let id = window::Id::unique();
        log::info!(
            "bars: spawning {namespace} for {monitor} (generation {})",
            self.bar_generation
        );
        self.bars.insert(
            id,
            BarRecord {
                monitor: monitor.clone(),
                namespace: namespace.clone(),
                verified: false,
                attempts: 0,
            },
        );
        Task::done(Message::NewLayerShell {
            settings: NewLayerShellSettings {
                anchor: Anchor::Left | Anchor::Top | Anchor::Bottom,
                layer: Layer::Top,
                exclusive_zone: Some(i32::try_from(style::BAR_WIDTH).unwrap_or(54)),
                size: Some((style::BAR_WIDTH, 0)),
                output_option: OutputOption::OutputName(monitor),
                keyboard_interactivity: KeyboardInteractivity::None,
                namespace: Some(namespace),
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

    /// Reconcile bars against what the compositor says is on screen.
    ///
    /// `observed` is the `j/layers` answer, or `None` if the query failed.
    /// Enforces three invariants over the connected monitors:
    ///
    /// 1. Every connected monitor has a bar on it.
    /// 2. No bar is left over for a disconnected monitor.
    /// 3. No two bars share a monitor.
    ///
    /// The previous version checked these against its own tracking map, which
    /// is exactly the thing that goes wrong — so it reported success in every
    /// broken state. Everything here is driven by `observed` instead.
    fn reconcile_bars(&mut self, observed: Option<&services::hyprland::LayerMap>) -> Task<Message> {
        let expected: std::collections::HashSet<String> =
            self.monitor_geoms.keys().cloned().collect();
        let plan = plan_from_observation(observed, &expected, &self.bars);

        let mut tasks = Vec::new();

        for id in &plan.verified {
            if let Some(record) = self.bars.get_mut(id) {
                if !record.verified {
                    log::info!("bars: {} confirmed on {}", record.namespace, record.monitor);
                }
                record.verified = true;
                record.attempts = 0;
                // Something is working; stop backing off.
                self.verify_backoff = VERIFY_DELAY;
            }
        }
        for id in &plan.pending {
            if let Some(record) = self.bars.get_mut(id) {
                record.attempts = record.attempts.saturating_add(1);
            }
        }
        for (id, reason) in &plan.close {
            if let Some(record) = self.bars.remove(id) {
                log::warn!(
                    "bars: closing {} ({reason}); wanted {}",
                    record.namespace,
                    record.monitor
                );
            }
            tasks.push(close_window(*id));
        }
        for (id, reason) in &plan.forget {
            if let Some(record) = self.bars.remove(id) {
                log::warn!("bars: forgetting {} ({reason})", record.namespace);
            }
            // Deliberately no `close_window`: the surface is already gone, and
            // a close for an unknown id is dropped by iced_layershell anyway.
            self.grow_verify_backoff();
        }
        for monitor in &plan.drop_state_for {
            self.drop_bar_state(monitor);
        }
        if let Some(monitor) = plan.spawn {
            // One spawn per pass, on purpose. Batching several put them all
            // into one `Task::batch`, which layershellev drains inside
            // `process_window_state` — a context that cannot dispatch the
            // wayland queue at all, so every spawn resolved its output name
            // against the same frozen cache. When that cache was cold they all
            // missed together and stacked on the focused monitor. Spawning one
            // at a time and verifying in between makes that impossible.
            tasks.push(self.spawn_bar_for(monitor));
        }

        self.log_bar_invariants(observed, &expected);

        // Keep verifying while anything is unconfirmed or uncovered. Once every
        // monitor has a verified bar this stops scheduling, so a settled
        // multi-monitor setup costs nothing.
        if self.bars_need_verification(&expected) {
            tasks.push(self.verify_bars_soon());
        }

        Task::batch(tasks)
    }

    /// Whether another verification pass is warranted.
    fn bars_need_verification(&self, expected: &std::collections::HashSet<String>) -> bool {
        let covered: std::collections::HashSet<&str> = self
            .bars
            .values()
            .filter(|r| r.verified)
            .map(|r| r.monitor.as_str())
            .collect();
        self.bars.values().any(|r| !r.verified)
            || expected.iter().any(|m| !covered.contains(m.as_str()))
    }

    /// Back off after a spawn failed to appear, so a compositor that will not
    /// place our surface is retried at a decreasing rate rather than hammered.
    fn grow_verify_backoff(&mut self) {
        self.verify_backoff = self
            .verify_backoff
            .saturating_mul(2)
            .min(MAX_VERIFY_BACKOFF);
    }

    /// Report invariant violations against the *observation*, always — not
    /// against our own tracking, and not only in debug builds. The previous
    /// check was self-referential and `#[cfg(debug_assertions)]`, so a release
    /// build said nothing while bars were visibly stacked on one screen.
    fn log_bar_invariants(
        &self,
        observed: Option<&services::hyprland::LayerMap>,
        expected: &std::collections::HashSet<String>,
    ) {
        let Some(observed) = observed else {
            return;
        };
        let ours: std::collections::HashSet<&str> =
            self.bars.values().map(|r| r.namespace.as_str()).collect();
        let mut per_monitor: HashMap<&str, usize> = HashMap::new();
        for (monitor, namespaces) in observed {
            let count = namespaces
                .iter()
                .filter(|ns| ours.contains(ns.as_str()))
                .count();
            if count > 0 {
                per_monitor.insert(monitor.as_str(), count);
            }
        }
        for (monitor, count) in &per_monitor {
            if *count > 1 {
                log::error!("bar invariant: {count} bars observed on monitor {monitor}");
            }
            if !expected.contains(*monitor) {
                log::error!("bar invariant: bar observed on unexpected monitor {monitor}");
            }
        }
        for monitor in expected {
            if !per_monitor.contains_key(monitor.as_str()) {
                // Normal while a spawn is still in flight; only a settled
                // state with no bar is a real violation, which the repeated
                // verification passes will surface as a persistent message.
                log::debug!("bars: monitor {monitor} has no bar yet");
            }
        }
    }

    /// Handle a window closed by the compositor. Layer surfaces are torn down
    /// when their `wl_output` disappears (monitor disconnect, DPMS, …).
    ///
    /// This is only an optimisation now: it lets us react immediately instead
    /// of waiting for the next verification pass. It is explicitly *not* the
    /// liveness signal, because for exactly the surfaces most likely to die —
    /// the ones bound to an output that just vanished — layershellev removes
    /// the unit before dispatching `Closed`, and `handle_closed_event` then
    /// early-returns on the unknown id, so the event never arrives at all.
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

        // Unknown ids are surfaces whose tracking we already cleared; they must
        // stay no-ops. Treating one as a bar is what let a panel or popup close
        // masquerade as "the bar died" and trigger a spurious respawn.
        if let Some(record) = self.bars.remove(&id) {
            log::info!(
                "bars: {} on {} was closed by the compositor",
                record.namespace,
                record.monitor
            );
            if !self.bars.values().any(|r| r.monitor == record.monitor) {
                self.drop_bar_state(&record.monitor);
            }
            return self.verify_bars_soon();
        }
        Task::none()
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
        // Every bar surface is created by us with a known id, so there is no
        // longer any id to guess at here — which is what the old lazy
        // "first unknown id must be the initial bar" capture was doing, and
        // what let a closing panel be adopted as the initial bar.
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
        if !self.audio.available {
            // Optimistically moving the slider with no live PipeWire behind it
            // is exactly the lie this flag exists to prevent: the command is
            // dropped and no correcting `AudioInfo` ever arrives.
            log::warn!("audio: ignoring volume change, PipeWire is unavailable");
            return Task::none();
        }
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
        // A bar with no monitor is not a state we can place a panel from, and
        // guessing an output is what the `LastOutput` fallback used to do.
        // Every bar surface is now tracked with its monitor, so this only fires
        // if the bar was closed between render and hover.
        let Some(monitor) = monitor else {
            log::warn!("panels: not opening {kind:?}, its bar has no known monitor");
            return Task::none();
        };
        let close = self.close_all_panels();
        let (width, height) = self.panel_dimensions(kind);
        if let Some(setter) = kind.signal_setter() {
            setter(true);
        }
        let open = self
            .panels
            .entry(kind)
            .or_default()
            .open(kind, width, height, &monitor);
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

    /// Maximum popup height in logical pixels: 2/5 of the logical height of the
    /// monitor the popup is *on*, or a conservative 1080p-based fallback.
    ///
    /// Measuring `focused_monitor` instead was wrong in both directions: the
    /// popup did not live there, and every focus change re-fitted the surface
    /// against a screen it was not on. With a 4K focused monitor and a 768px
    /// host that produced a cap taller than the screen, and since the popup
    /// column has no scrollable the overflow summary itself fell off-screen.
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
            .notif_popup_monitor
            .as_deref()
            .or(self.focused_monitor.as_deref())
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

    /// The monitor the popup belongs on: the focused one while it is still
    /// connected, otherwise any connected monitor (lowest name, so the choice
    /// is stable rather than flipping on `HashMap` order).
    fn popup_monitor(&self) -> Option<String> {
        if let Some(focused) = self.focused_monitor.as_deref() {
            if self.monitor_geoms.contains_key(focused) {
                return Some(focused.to_string());
            }
        }
        let mut names: Vec<&String> = self.monitor_geoms.keys().collect();
        names.sort();
        names.first().map(|name| (*name).clone())
    }

    fn ensure_popup_window(&mut self) -> Task<Message> {
        if self.popup_notifications.is_empty() {
            return Task::none();
        }
        let Some(target) = self.popup_monitor() else {
            log::warn!("notifications: no connected monitor to place the popup on");
            return Task::none();
        };

        // Moving the popup needs a new surface: `SizeChange` can only resize,
        // never re-place.
        if self.notif_popup_id.is_some() && self.notif_popup_monitor.as_deref() != Some(&target) {
            let close = self
                .notif_popup_id
                .take()
                .map_or_else(Task::none, close_window);
            self.notif_popup_monitor = None;
            return Task::batch([close, self.create_popup_window(target)]);
        }

        if let Some(id) = self.notif_popup_id {
            // Resize existing window to fit current notification layout
            let (visible, overflow) = self.popup_fit();
            return Task::done(Message::SizeChange {
                id,
                size: (
                    style::NOTIF_WIDTH,
                    style::notif_popup_height(visible, overflow),
                ),
            });
        }
        self.create_popup_window(target)
    }

    /// Create the popup surface pinned to `monitor`.
    ///
    /// The output is explicit because the default `OutputOption::None`
    /// resolves through `current_surface` — written by the last surface
    /// created *and* by the last pointer button press, and never cleared when
    /// a surface is removed — then falls back to `outputs.first()`. So popups
    /// landed on an arbitrary monitor, never the tracked focused one, while
    /// the height cap was computed from the focused monitor's geometry.
    fn create_popup_window(&mut self, monitor: String) -> Task<Message> {
        let id = window::Id::unique();
        self.notif_popup_id = Some(id);
        // Set before measuring: `popup_max_height` derives the cap from the
        // monitor the surface is actually on.
        self.notif_popup_monitor = Some(monitor.clone());
        let (visible, overflow) = self.popup_fit();
        let height = style::notif_popup_height(visible, overflow);
        Task::done(Message::NewLayerShell {
            settings: NewLayerShellSettings {
                anchor: Anchor::Right | Anchor::Top,
                layer: Layer::Overlay,
                exclusive_zone: Some(-1),
                size: Some((style::NOTIF_WIDTH, height)),
                margin: Some((8, 8, 8, 8)),
                keyboard_interactivity: KeyboardInteractivity::None,
                output_option: OutputOption::OutputName(monitor),
                namespace: Some(POPUP_NAMESPACE.to_string()),
                ..NewLayerShellSettings::default()
            },
            id,
        })
    }

    /// Drop the notification popup so the next one is created on a live output.
    ///
    /// The popup owns a concrete `wl_output`, so when that output disappears
    /// layershellev removes the unit before dispatching `Closed` and the event
    /// is swallowed — leaving `notif_popup_id` pointing at a dead surface
    /// forever. `ensure_popup_window` would then only ever emit `SizeChange`,
    /// which is dropped for a dead unit, so every subsequent notification was
    /// accepted over D-Bus and silently discarded. Clearing the id is what
    /// breaks that.
    fn invalidate_popup(&mut self) -> Task<Message> {
        let Some(id) = self.notif_popup_id.take() else {
            return Task::none();
        };
        self.notif_popup_monitor = None;
        // Ask for a close in case the surface is in fact still alive; a close
        // for an id iced_layershell no longer knows is dropped harmlessly.
        let close = close_window(id);
        if self.popup_notifications.is_empty() {
            return close;
        }
        Task::batch([close, self.ensure_popup_window()])
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

/// One bar surface we have asked the compositor for.
///
/// The distinction that matters: `monitor` is what we *requested*, `verified`
/// is whether the compositor was ever observed agreeing. Treating the request
/// as the truth is what produced bars stacked on one screen while the app
/// believed they were spread across all of them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BarRecord {
    /// Monitor this bar was spawned for.
    monitor: String,
    /// Unique layer-shell namespace, our handle for matching `j/layers` output
    /// back to this specific surface.
    namespace: String,
    /// Set once this namespace was observed on `monitor`.
    verified: bool,
    /// Verification passes survived without being observed. Bounded by
    /// `VERIFY_GRACE_PASSES` so a surface that never maps is eventually
    /// replaced rather than masking its monitor forever.
    attempts: u32,
}

/// What `plan_from_observation` decided. Every field is sorted so the plan is
/// deterministic and directly comparable in tests.
#[derive(Debug, Default, PartialEq, Eq)]
struct BarPlan {
    /// Surfaces to close, with the reason, because they are on the wrong
    /// monitor or their monitor is gone.
    close: Vec<(window::Id, &'static str)>,
    /// Records to drop without closing: the surface is already gone.
    forget: Vec<(window::Id, &'static str)>,
    /// Records observed where we asked for them.
    verified: Vec<window::Id>,
    /// Records still within their grace window; the caller counts an attempt.
    pending: Vec<window::Id>,
    /// Monitors whose per-monitor state should be dropped.
    drop_state_for: Vec<String>,
    /// The single monitor to spawn a bar for this pass, if any.
    spawn: Option<String>,
}

/// Decide what to do about the bars, given what the compositor reports.
///
/// Pure, so the whole state machine is testable without a compositor: this is
/// what the old belief-only planner could not offer, because the bugs it needed
/// to catch all lived in the gap between the tracking map and reality.
///
/// `observed` maps monitor name to the layer namespaces mapped there. `None`
/// means the query failed.
fn plan_from_observation(
    observed: Option<&services::hyprland::LayerMap>,
    expected: &std::collections::HashSet<String>,
    tracked: &HashMap<window::Id, BarRecord>,
) -> BarPlan {
    let mut plan = BarPlan::default();

    // Two no-op guards, both load-bearing. Without an observation we know
    // nothing, and acting on nothing is how a transient IPC failure used to
    // close every bar. An empty `expected` is the same story from the other
    // direction: "we could not read the monitor list" must never be actioned
    // as "there are no monitors".
    let (Some(observed), false) = (observed, expected.is_empty()) else {
        return plan;
    };

    // Where each namespace actually is, according to the compositor.
    let mut location: HashMap<&str, &str> = HashMap::new();
    for (monitor, namespaces) in observed {
        for namespace in namespaces {
            location.insert(namespace.as_str(), monitor.as_str());
        }
    }

    // Walk records in id order so the plan does not depend on HashMap order.
    let mut records: Vec<(&window::Id, &BarRecord)> = tracked.iter().collect();
    records.sort_by_key(|(id, _)| **id);

    // Monitors that end this pass with a bar we trust to be there.
    let mut covered: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for (id, record) in records {
        let wanted_gone = !expected.contains(&record.monitor);
        match location.get(record.namespace.as_str()) {
            // Observed exactly where we asked.
            Some(actual) if *actual == record.monitor => {
                if wanted_gone {
                    plan.close.push((*id, "monitor disconnected"));
                } else if covered.insert(actual) {
                    plan.verified.push(*id);
                } else {
                    // Another bar already holds this monitor. Duplicates are
                    // resolved by id order so the choice is stable.
                    plan.close.push((*id, "duplicate on monitor"));
                }
            }
            // Observed somewhere else: `OutputName` fell back to the focused
            // output and nothing told us. This is the flagship bug, and the
            // only reason it is fixable is that we can see it here.
            Some(_) => plan.close.push((*id, "landed on the wrong monitor")),
            // Not mapped anywhere.
            None => {
                if wanted_gone {
                    plan.forget.push((*id, "monitor disconnected"));
                } else if record.verified {
                    // It was there and is not any more: the surface died
                    // without a usable `Closed` event, which is precisely the
                    // lost-close case that used to strand a monitor forever.
                    plan.forget.push((*id, "surface vanished"));
                } else if record.attempts >= VERIFY_GRACE_PASSES {
                    plan.forget.push((*id, "never appeared"));
                } else {
                    // Still mapping. Hold its monitor so we do not spawn a
                    // second bar on top of a surface that is on its way.
                    plan.pending.push(*id);
                    covered.insert(record.monitor.as_str());
                }
            }
        }
    }

    // Per-monitor state belongs to monitors that no longer keep a bar.
    let mut dropped: Vec<String> = tracked
        .values()
        .map(|r| r.monitor.clone())
        .filter(|m| !covered.contains(m.as_str()))
        .collect();
    dropped.sort();
    dropped.dedup();
    plan.drop_state_for = dropped;

    // One uncovered monitor per pass, lowest name first for determinism.
    let mut uncovered: Vec<&String> = expected
        .iter()
        .filter(|m| !covered.contains(m.as_str()))
        .collect();
    uncovered.sort();
    plan.spawn = uncovered.first().map(|m| (*m).clone());

    plan.close.sort();
    plan.forget.sort();
    plan.verified.sort();
    plan.pending.sort();
    plan
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod reconcile_tests {
    use super::{plan_from_observation, BarPlan, BarRecord, VERIFY_GRACE_PASSES};
    use iced::window;
    use std::collections::{HashMap, HashSet};

    fn expected<const N: usize>(monitors: [&str; N]) -> HashSet<String> {
        monitors.iter().map(|m| (*m).to_string()).collect()
    }

    /// A tracking map from `(monitor, namespace, verified)` triples.
    fn tracked<const N: usize>(
        entries: [(window::Id, &str, &str, bool); N],
    ) -> HashMap<window::Id, BarRecord> {
        entries
            .into_iter()
            .map(|(id, monitor, namespace, verified)| {
                (
                    id,
                    BarRecord {
                        monitor: monitor.to_string(),
                        namespace: namespace.to_string(),
                        verified,
                        attempts: 0,
                    },
                )
            })
            .collect()
    }

    /// A `j/layers`-shaped observation.
    fn observed<const N: usize>(
        entries: [(&str, &[&str]); N],
    ) -> super::services::hyprland::LayerMap {
        entries
            .into_iter()
            .map(|(monitor, namespaces)| {
                (
                    monitor.to_string(),
                    namespaces.iter().map(|n| (*n).to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn a_failed_observation_changes_nothing() {
        // The single most damaging old behaviour: an IPC failure read as "no
        // monitors" closed every bar, which under StartMode::Active emptied
        // `units` and killed the process.
        let plan = plan_from_observation(None, &expected(["DP-1"]), &HashMap::new());
        assert_eq!(plan, BarPlan::default());
    }

    #[test]
    fn an_empty_monitor_set_changes_nothing() {
        let a = window::Id::unique();
        let plan = plan_from_observation(
            Some(&observed([("DP-1", &["obayebar-bar-1"][..])])),
            &HashSet::new(),
            &tracked([(a, "DP-1", "obayebar-bar-1", true)]),
        );
        assert_eq!(plan, BarPlan::default());
    }

    #[test]
    fn an_empty_setup_spawns_for_one_monitor() {
        let plan = plan_from_observation(Some(&observed([])), &expected(["DP-1"]), &HashMap::new());
        assert_eq!(plan.spawn.as_deref(), Some("DP-1"));
        assert!(plan.close.is_empty());
    }

    #[test]
    fn spawns_are_serialised_one_per_pass() {
        // Batching them is what made a cold output-name cache stack every bar
        // on the focused monitor: they all resolved against one frozen
        // snapshot inside a context that cannot dispatch the wayland queue.
        let plan = plan_from_observation(
            Some(&observed([])),
            &expected(["DP-1", "DP-2", "HDMI-A-1"]),
            &HashMap::new(),
        );
        assert_eq!(plan.spawn.as_deref(), Some("DP-1"));
    }

    #[test]
    fn a_bar_observed_where_requested_is_verified() {
        let a = window::Id::unique();
        let plan = plan_from_observation(
            Some(&observed([("DP-1", &["obayebar-bar-1"][..])])),
            &expected(["DP-1"]),
            &tracked([(a, "DP-1", "obayebar-bar-1", false)]),
        );
        assert_eq!(plan.verified, vec![a]);
        assert_eq!(plan.spawn, None);
        assert!(plan.close.is_empty());
    }

    #[test]
    fn a_bar_on_the_wrong_monitor_is_closed_and_respawned() {
        // The OutputName silent fallback. Previously invisible and permanent:
        // the app kept believing the bar was on DP-2 forever.
        let a = window::Id::unique();
        let plan = plan_from_observation(
            Some(&observed([("DP-1", &["obayebar-bar-1"][..])])),
            &expected(["DP-1", "DP-2"]),
            &tracked([(a, "DP-2", "obayebar-bar-1", false)]),
        );
        assert_eq!(plan.close, vec![(a, "landed on the wrong monitor")]);
        // DP-1 has no bar of ours that we asked for, DP-2 lost its only
        // candidate — one of them gets this pass.
        assert!(plan.spawn.is_some());
    }

    #[test]
    fn two_bars_on_one_monitor_leaves_exactly_one() {
        let a = window::Id::unique();
        let b = window::Id::unique();
        let plan = plan_from_observation(
            Some(&observed([(
                "DP-1",
                &["obayebar-bar-1", "obayebar-bar-2"][..],
            )])),
            &expected(["DP-1"]),
            &tracked([
                (a, "DP-1", "obayebar-bar-1", true),
                (b, "DP-1", "obayebar-bar-2", true),
            ]),
        );
        let (kept, dropped) = if a < b { (a, b) } else { (b, a) };
        assert_eq!(plan.verified, vec![kept]);
        assert_eq!(plan.close, vec![(dropped, "duplicate on monitor")]);
        assert_eq!(plan.spawn, None);
    }

    #[test]
    fn a_disconnected_monitor_closes_its_observed_bar() {
        let a = window::Id::unique();
        let plan = plan_from_observation(
            Some(&observed([("DP-2", &["obayebar-bar-1"][..])])),
            &expected(["DP-1"]),
            &tracked([(a, "DP-2", "obayebar-bar-1", true)]),
        );
        assert_eq!(plan.close, vec![(a, "monitor disconnected")]);
        assert_eq!(plan.drop_state_for, vec!["DP-2".to_string()]);
        assert_eq!(plan.spawn.as_deref(), Some("DP-1"));
    }

    #[test]
    fn a_disconnected_monitor_forgets_its_unmapped_bar() {
        let a = window::Id::unique();
        let plan = plan_from_observation(
            Some(&observed([])),
            &expected(["DP-1"]),
            &tracked([(a, "DP-2", "obayebar-bar-1", true)]),
        );
        assert_eq!(plan.forget, vec![(a, "monitor disconnected")]);
        assert!(plan.close.is_empty());
    }

    #[test]
    fn a_vanished_verified_bar_is_forgotten_and_respawned() {
        // The lost-Closed case: layershellev removes the unit before
        // dispatching Closed, so the app never hears about it. Observation is
        // what catches it; without this the monitor stayed masked forever.
        let a = window::Id::unique();
        let plan = plan_from_observation(
            Some(&observed([("DP-1", &[][..])])),
            &expected(["DP-1"]),
            &tracked([(a, "DP-1", "obayebar-bar-1", true)]),
        );
        assert_eq!(plan.forget, vec![(a, "surface vanished")]);
        assert_eq!(plan.spawn.as_deref(), Some("DP-1"));
        assert_eq!(plan.drop_state_for, vec!["DP-1".to_string()]);
    }

    #[test]
    fn a_freshly_spawned_bar_gets_grace_and_holds_its_monitor() {
        // Verifying immediately after a spawn sees nothing, so an unverified
        // record must not be read as failure — otherwise every spawn is
        // instantly replaced and the bar never settles.
        let a = window::Id::unique();
        let plan = plan_from_observation(
            Some(&observed([])),
            &expected(["DP-1"]),
            &tracked([(a, "DP-1", "obayebar-bar-1", false)]),
        );
        assert_eq!(plan.pending, vec![a]);
        assert_eq!(plan.spawn, None, "must not double-spawn while mapping");
        assert!(plan.forget.is_empty());
    }

    #[test]
    fn a_bar_that_never_appears_is_replaced_after_the_grace_window() {
        let a = window::Id::unique();
        let mut map = tracked([(a, "DP-1", "obayebar-bar-1", false)]);
        map.entry(a)
            .and_modify(|r| r.attempts = VERIFY_GRACE_PASSES);
        let plan = plan_from_observation(Some(&observed([])), &expected(["DP-1"]), &map);
        assert_eq!(plan.forget, vec![(a, "never appeared")]);
        assert_eq!(plan.spawn.as_deref(), Some("DP-1"));
    }

    #[test]
    fn foreign_layer_surfaces_are_ignored() {
        // Other clients' layers share j/layers with ours; only our namespaces
        // may influence the plan.
        let a = window::Id::unique();
        let plan = plan_from_observation(
            Some(&observed([(
                "DP-1",
                &["waybar", "obayebar-bar-1", "gtk-layer-shell"][..],
            )])),
            &expected(["DP-1"]),
            &tracked([(a, "DP-1", "obayebar-bar-1", true)]),
        );
        assert_eq!(plan.verified, vec![a]);
        assert_eq!(plan.spawn, None);
        assert!(plan.close.is_empty());
    }

    #[test]
    fn a_settled_multi_monitor_setup_is_a_no_op() {
        // Idempotence: the steady state must produce an empty plan, or the
        // loop would churn surfaces forever.
        let a = window::Id::unique();
        let b = window::Id::unique();
        let plan = plan_from_observation(
            Some(&observed([
                ("DP-1", &["obayebar-bar-1"][..]),
                ("DP-2", &["obayebar-bar-2"][..]),
            ])),
            &expected(["DP-1", "DP-2"]),
            &tracked([
                (a, "DP-1", "obayebar-bar-1", true),
                (b, "DP-2", "obayebar-bar-2", true),
            ]),
        );
        assert_eq!(plan.spawn, None);
        assert!(plan.close.is_empty());
        assert!(plan.forget.is_empty());
        assert!(plan.drop_state_for.is_empty());
        assert_eq!(plan.verified.len(), 2);
    }

    #[test]
    fn panel_and_popup_namespaces_never_count_as_bars() {
        // Our own non-bar surfaces are in j/layers too. Counting one as a bar
        // would mask a monitor that has no bar at all.
        let plan = plan_from_observation(
            Some(&observed([(
                "DP-1",
                &["obayebar-panel-audio", "obayebar-notifications"][..],
            )])),
            &expected(["DP-1"]),
            &HashMap::new(),
        );
        assert_eq!(plan.spawn.as_deref(), Some("DP-1"));
    }

    #[test]
    fn rapid_add_remove_add_converges() {
        // DP-2 disappears and comes back while its bar was still unverified.
        // The stale record must not mask the returning monitor.
        let a = window::Id::unique();
        let mut map = tracked([(a, "DP-2", "obayebar-bar-1", false)]);
        map.entry(a)
            .and_modify(|r| r.attempts = VERIFY_GRACE_PASSES);

        // Gone: forget it, and DP-1 is the only monitor left to serve.
        let gone = plan_from_observation(Some(&observed([])), &expected(["DP-1"]), &map);
        assert_eq!(gone.forget, vec![(a, "monitor disconnected")]);

        // Back, with nothing tracked: it gets a fresh spawn.
        let back = plan_from_observation(
            Some(&observed([("DP-1", &["obayebar-bar-2"][..])])),
            &expected(["DP-1", "DP-2"]),
            &HashMap::new(),
        );
        assert_eq!(back.spawn.as_deref(), Some("DP-1"));
    }
}
