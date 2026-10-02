mod bar;
mod bars;
mod calendar;
mod config;
mod control;
mod launcher;
mod media;
mod notifications;
mod panel;
mod services;
mod style;

use launcher::Launcher;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bar::workspaces::SpringState;
use iced::event::Event;
use iced::widget::canvas;
use iced::window;
use iced::{Element, Subscription, Task, Theme};
use iced_layershell::reexport::{
    Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings, OutputOption,
};
use iced_layershell::settings::{LayerShellSettings, Settings, StartMode};
use iced_layershell::to_layer_message;
use obayebar_core::hypr::MonitorGeom;
use panel::PanelKind;
use services::audio::{AudioCommand, AudioInfo};
use services::battery::BatteryInfo;
use services::bluetooth::BluetoothInfo;
use services::gitlab::GitlabInfo;
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

/// Namespace for the notification popup surface, so `j/layers` can tell it
/// apart from a bar, and a `layerrule` can target one without the other.
const POPUP_NAMESPACE: &str = "obayebar-notifications";

/// How long after the pointer leaves a bar trigger or a panel before the panel
/// is dismissed. Long enough to cross the gap between the two surfaces, short
/// enough that a deliberate move away feels immediate.
const PANEL_GRACE: std::time::Duration = std::time::Duration::from_millis(150);

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
    /// `Some(true)` for `--media`, `Some(false)` for `--no-media`; `None`
    /// means "use config".
    media_enable: Option<bool>,
}

/// Printed for `-h`/`--help`, and pasted verbatim into the README.
const USAGE: &str = "\
obayebar [OPTIONS]

  --gitlab              Show the GitLab todos module on the bar
  --gitlab-url <URL>    Base URL of the GitLab instance (overrides config / env)
  --media               Show the media (MPRIS) module (overrides config)
  --no-media            Leave the media module out entirely
  -h, --help            Print this help
  -V, --version         Print version

Persistent settings can also be placed in $XDG_CONFIG_HOME/obayebar/config.toml
(see [gitlab].enable / [gitlab].url / [media].enable).";

fn print_usage() {
    println!("{USAGE}");
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
            "--media" | "--no-media" => {
                args.media_enable = Some(arg == "--media");
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
        media_enable: args.media_enable,
    };
    let file = config::Config::load();
    config::install(&file, &cli);

    // Default to info so the command and service logs are actually visible;
    // RUST_LOG still overrides, both up to debug and back down to error.
    let logger =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).build();
    let max_level = logger.filter();
    log::set_boxed_logger(Box::new(FatalErrorLogger { inner: logger }))
        .map(|()| log::set_max_level(max_level))
        .ok();

    // After the logger, because a slice name the config got wrong is reported
    // rather than obeyed, and before the first launch, because a program takes
    // its slice when it is built.
    obayebar_core::spawn::install(&file.spawn);

    // The clipboard worker stays on now that the launcher lives here: its
    // search field is keyboard-interactive, so Ctrl+V in it is a real
    // paste. That is what the old standalone launcher process gave, and
    // `disable_clipboard()` here would quietly take it away.

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
    /// Every bar surface we have asked the compositor for, and the state the
    /// reconcile loop needs to keep that in sync with reality. See
    /// [`bars::BarFleet`].
    bars: bars::BarFleet,
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
    /// Where the pointer is relative to the panels, which is what decides
    /// dismissal.
    panel_pointer: PanelPointer,
    open_intent: panel::OpenIntent,
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
    pub calendar: calendar::Pager,
    pub battery: BatteryInfo,
    /// Behind an `Arc` because `bar::view` clones it on every frame — and the
    /// workspace spring drives 60 frames a second per monitor while animating,
    /// with a scanned access-point list inside. The `lazy` widgets downstream
    /// then usually decide nothing changed and drop it.
    pub network: Arc<NetworkInfo>,
    pub connecting_ssid: Option<String>,
    pub audio: Arc<AudioInfo>,
    pub bluetooth: Arc<BluetoothInfo>,
    pub sysinfo: Arc<SysInfo>,
    pub tray_items: Arc<Vec<TrayItemInfo>>,
    /// `None` when `[media].enable` is off: no service, no entry, no panel.
    pub media: Option<media::MediaState>,
    pub popup_notifications: Vec<NotificationData>,
    pub hovered_notif_id: Option<u32>,

    /// The launcher's state, kept between openings: its entry list is scanned
    /// once at startup and refreshed by a filesystem watch, so showing the
    /// surface has no work to do.
    launcher: Launcher,
    /// The launcher surface, while one is up.
    launcher_window: Option<window::Id>,
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
    /// A press on a bar trigger: open its panel at once.
    PanelOpen(panel::OpenRequest),
    /// The pointer arrived on a bar trigger. Opens its panel once the pointer
    /// has rested there for the open delay, or at once if a panel is up.
    PanelHovered(panel::OpenRequest),
    /// The open delay of this hover elapsed.
    PanelOpenDelayElapsed(panel::Ticket),
    Calendar(calendar::Paging),
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
    LayersObserved(Option<obayebar_core::hypr::LayerMap>),
    NetworkConnect(String),
    /// Outcome of the `NetworkManager` connect request itself.
    NetworkConnectDone(Result<(), String>),
    /// `CONNECT_TIMEOUT` elapsed for this SSID; clears a spinner that no
    /// `NetworkManager` state change would ever clear.
    NetworkConnectTimedOut(String),
    NetworkDisconnect,
    /// The pointer left a bar trigger (icon). Arms a grace close.
    PanelPointerLeftTrigger(PanelKind),
    /// The pointer left a panel surface. Arms a grace close.
    PanelPointerLeftPanel(PanelKind),
    /// The pointer entered a panel surface, cancelling any pending close.
    PanelPointerEntered(PanelKind),
    /// The grace window elapsed; close if the pointer is still nowhere near.
    PanelGraceElapsed,
    /// Close every panel immediately, regardless of the pointer.
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
    /// A message from the media service or the media panel's widgets.
    Media(media::Message),
    WindowClosed(window::Id),
    /// A line arrived on the control socket, from `obayebar-launcher`.
    Control(obayebar_core::control::BarCommand),
    /// A message from the launcher's own widgets.
    Launcher(launcher::Message),
    /// A freshly scanned application list, from the startup scan or from the
    /// filesystem watch that follows it.
    LauncherIndex(launcher::desktop_entry::Index),
    /// Keyboard and window events, routed to the launcher when they belong to
    /// its surface. The search field has focus, so Escape, the arrows and
    /// Enter would otherwise be swallowed by the text input.
    LauncherEvent(window::Id, Event),
}

impl App {
    fn new() -> (Self, Task<Message>) {
        (
            Self {
                bars: bars::BarFleet::new(),
                ws_spring: HashMap::new(),
                ws_cache: HashMap::new(),
                ws_cache_fallback: canvas::Cache::default(),
                vector_font: style::load_vector_font(),
                notif_popup_id: None,
                notif_popup_monitor: None,
                panels: HashMap::new(),
                panel_pointer: PanelPointer::default(),
                open_intent: panel::OpenIntent::default(),
                gitlab_enabled: config::resolved().gitlab_enable(),
                gitlab: GitlabInfo::default(),
                gitlab_token_input: String::new(),
                workspaces: Vec::new(),
                active_workspaces: HashMap::new(),
                monitor_geoms: HashMap::new(),
                focused_monitor: None,
                active_window: None,
                time: chrono::Local::now(),
                calendar: calendar::Pager::default(),
                battery: BatteryInfo::default(),
                network: Arc::new(NetworkInfo::default()),
                connecting_ssid: None,
                audio: Arc::new(AudioInfo::default()),
                bluetooth: Arc::new(BluetoothInfo::default()),
                sysinfo: Arc::new(SysInfo::default()),
                tray_items: Arc::new(Vec::new()),
                media: config::resolved().media_enable().then(|| {
                    media::MediaState::new(
                        std::time::Instant::now(),
                        config::resolved().media_show_when_idle(),
                    )
                }),
                popup_notifications: Vec::new(),
                hovered_notif_id: None,
                launcher: Launcher::new(),
                launcher_window: None,
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
    /// An exact lookup, with no fallback: an unknown id must never be read
    /// as belonging to any particular monitor, or a panel or popup surface
    /// could be rendered as a bar on the wrong one.
    fn monitor_for_bar(&self, id: window::Id) -> Option<&str> {
        self.bars.monitor_for(id)
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

    fn update(&mut self, message: Message) -> Task<Message> {
        if let Some(media) = self.media.as_mut() {
            media.tick(std::time::Instant::now());
        }
        let task = self.handle_message(message);
        self.sync_panel_signals();
        // Any message can change what a panel renders, so refit here rather
        // than in each service arm. `Panel::resize` no-ops when
        // the size is unchanged, so this costs nothing in the common case.
        let resize = self.resize_open_panel();
        Task::batch([task, resize])
    }

    /// Refit the open panel's surface to its current content.
    fn resize_open_panel(&mut self) -> Task<Message> {
        let Some(kind) = self
            .panels
            .iter()
            .find_map(|(kind, panel)| panel.is_open().then_some(*kind))
        else {
            return Task::none();
        };
        let height = self.panel_height(kind);
        self.panels
            .get_mut(&kind)
            .map_or_else(Task::none, |panel| panel.resize(height))
    }

    /// Derive each service's cadence signal from whether its panel is actually
    /// open, rather than latching it at open/close time.
    ///
    /// A panel surface can die without a usable `Closed` event, so latching
    /// the signal would risk leaving a service stuck polling at panel
    /// cadence indefinitely. `PanelSignal::set` only notifies on a change, so
    /// re-deriving it here after every message is cheap.
    fn sync_panel_signals(&self) {
        for (kind, panel) in &self.panels {
            if let Some(setter) = kind.signal_setter() {
                setter(panel.is_open());
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle_message(&mut self, message: Message) -> Task<Message> {
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
                if *self.network != info {
                    self.network = Arc::new(info);
                }
                Task::none()
            }
            Message::Audio(info) => {
                if *self.audio != info {
                    self.audio = Arc::new(info);
                }
                Task::none()
            }
            Message::Bluetooth(info) => {
                if *self.bluetooth != info {
                    self.bluetooth = Arc::new(info);
                }
                Task::none()
            }
            Message::SysInfo(info) => {
                if *self.sysinfo != info {
                    self.sysinfo = Arc::new(info);
                }
                Task::none()
            }
            Message::TrayItems(items) => {
                if *self.tray_items != items {
                    self.tray_items = Arc::new(items);
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
            Message::PanelOpen(request) => self.open_panel(request),
            Message::PanelHovered(request) => self.hover_panel(request),
            Message::PanelOpenDelayElapsed(ticket) => self.settle_open_delay(ticket),
            Message::Calendar(paging) => {
                self.calendar.apply(paging);
                Task::none()
            }
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
                {
                    // Optimistic update: clone-on-write is fine here, this is
                    // a user action rather than a per-frame path.
                    let network = Arc::make_mut(&mut self.network);
                    network.wifi_enabled = enabled;
                    if !enabled {
                        network.icon_name = style::ICON_WIFI_OFF;
                    }
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
            Message::PanelPointerEntered(kind) => {
                self.panel_pointer.entered_panel(kind);
                Task::none()
            }
            Message::PanelPointerLeftTrigger(kind) => {
                self.panel_pointer.left_trigger(kind);
                self.open_intent.left(kind);
                Self::arm_panel_grace()
            }
            Message::PanelPointerLeftPanel(kind) => {
                self.panel_pointer.left_panel(kind);
                Self::arm_panel_grace()
            }
            Message::PanelGraceElapsed => {
                // Close only if the pointer is on neither a trigger nor a
                // panel. Both are tracked separately and checked together so
                // the decision does not depend on the order the leave and the
                // enter happen to be dispatched in — which is fixed by window
                // id and therefore not something to rely on.
                if self.panel_pointer.is_away() {
                    self.close_all_panels()
                } else {
                    Task::none()
                }
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
                {
                    let audio = Arc::make_mut(&mut self.audio);
                    audio.muted = muted;
                    audio.icon_name = crate::services::audio::volume_icon(audio.volume, muted);
                }
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
            Message::Media(message) => self.update_media(message),
            Message::AudioOpenPavucontrol => {
                // A mixer the user opened from the bar has no business dying
                // when the bar restarts, so it goes out through the spawner
                // like everything else the bar starts for the user.
                if let Err(err) = obayebar_core::spawn::Program::new("pavucontrol")
                    .tag("pavucontrol")
                    .singleton(obayebar_core::spawn::OnCollision::Refuse)
                    .spawn()
                {
                    // `AlreadyRunning` lands here too: a second click while
                    // the mixer is up is a no-op, not a failure.
                    log::info!("audio: pavucontrol not started ({err})");
                }
                Task::none()
            }
            Message::LayersObserved(observed) => self.reconcile_bars(observed.as_ref()),
            Message::WindowClosed(id) => self.handle_window_closed(id),
            Message::Control(obayebar_core::control::BarCommand::LauncherToggle) => {
                self.toggle_launcher()
            }
            Message::Launcher(message) => {
                let response = self.launcher.update(message);
                self.apply_launcher_response(response)
            }
            Message::LauncherIndex(index) => self.adopt_launcher_index(index),
            Message::LauncherEvent(id, event) => {
                // Events for any other surface are not the launcher's
                // business, and one of them is `Unfocused` on a bar.
                if self.launcher_window != Some(id) {
                    return Task::none();
                }
                let response = self.launcher.handle_event(&event);
                self.apply_launcher_response(response)
            }
            _ => Task::none(),
        }
    }

    /// Show the launcher, or take it away if it is already up.
    fn toggle_launcher(&mut self) -> Task<Message> {
        if self.launcher_window.is_some() {
            return self.close_launcher();
        }
        // Pinned to an output like every other surface here: the default
        // resolves through layershellev's "last output" state, which is
        // advanced by pointer presses and never cleared, so the launcher could
        // open on a screen the user is not looking at.
        let Some(monitor) = self.overlay_monitor() else {
            log::warn!("launcher: no connected monitor to open on");
            return Task::none();
        };
        if self.launcher.is_empty() {
            // Not fatal — the watch keeps scanning — but an empty launcher
            // looks like a broken one, so say why.
            log::warn!("launcher: opening with no entries discovered yet");
        }

        let id = window::Id::unique();
        self.launcher_window = Some(id);
        let reset = self.launcher.reset().map(Message::Launcher);
        // A panel hovering over the launcher would sit on top of it and keep
        // its service polling at panel cadence while the user types.
        let panels = self.close_all_panels();

        Task::batch([
            panels,
            Task::done(Message::NewLayerShell {
                settings: NewLayerShellSettings {
                    // All four edges so the dimmed backdrop covers the screen
                    // and a click outside the card lands on our surface.
                    anchor: Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
                    layer: Layer::Overlay,
                    exclusive_zone: Some(-1),
                    size: Some((launcher::LAUNCHER_WIDTH, launcher::LAUNCHER_HEIGHT)),
                    // The one surface in this process that takes the keyboard:
                    // it is a search field, and without an exclusive grab the
                    // compositor keeps sending keys to the focused window.
                    keyboard_interactivity: KeyboardInteractivity::Exclusive,
                    output_option: OutputOption::OutputName(monitor),
                    namespace: Some(launcher::NAMESPACE.to_string()),
                    ..NewLayerShellSettings::default()
                },
                id,
            }),
            reset,
        ])
    }

    fn close_launcher(&mut self) -> Task<Message> {
        self.launcher_window
            .take()
            .map_or_else(Task::none, close_window)
    }

    /// Run whatever the launcher asked for, and take its surface down if it
    /// said it was done.
    fn apply_launcher_response(&mut self, response: launcher::Response) -> Task<Message> {
        let task = response.task.map(Message::Launcher);
        if response.dismiss {
            return Task::batch([task, self.close_launcher()]);
        }
        task
    }

    /// Adopt a scanned application list and refresh the icons behind it.
    fn adopt_launcher_index(&mut self, index: launcher::desktop_entry::Index) -> Task<Message> {
        let icon_paths = index.icon_paths;
        self.launcher.set_entries(index.entries);

        // Decoding is the expensive half — up to ninety files, half of them
        // SVGs — so it happens off the UI thread, and the launcher draws with
        // whatever icons it already has until the new set lands.
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let live: std::collections::HashSet<&str> =
                        icon_paths.keys().map(String::as_str).collect();
                    // Uninstalling an application, or an upgrade that renames
                    // its entry, would otherwise leave its pixels cached
                    // forever.
                    launcher::icons::prune(&live);
                    launcher::icons::load(&icon_paths)
                })
                .await
                .unwrap_or_default()
            },
            |icons| Message::Launcher(launcher::Message::IconsLoaded(icons)),
        )
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
            self.bars.reset_backoff();
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
            tasks.push(self.close_launcher());
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

    /// Ask the compositor where our bars actually are, after a delay that
    /// grows if a spawn keeps failing to appear.
    ///
    /// The delay exists because a surface is not mapped the instant
    /// `NewLayerShell` is queued; verifying immediately would see nothing and
    /// conclude the spawn failed. At most one pass is ever in flight: a
    /// Hyprland hotplug asks for several in a burst, and `BarFleet` collapses
    /// them into one.
    fn verify_bars_soon(&mut self) -> Task<Message> {
        let Some(delay) = self.bars.begin_verify() else {
            return Task::none();
        };
        Task::perform(
            async move {
                tokio::time::sleep(delay).await;
                obayebar_core::hypr::fetch_layer_namespaces().await
            },
            Message::LayersObserved,
        )
    }

    /// Drop per-monitor workspace state — the surface is gone (compositor
    /// closed it) so its workspace cache/spring should not linger either.
    fn drop_bar_state(&mut self, monitor: &str) {
        self.ws_spring.remove(monitor);
        self.ws_cache.remove(monitor);
    }

    /// Reconcile bars against what the compositor says is on screen, and turn
    /// the outcome into tasks. The reconcile logic itself lives in
    /// [`bars::BarFleet::reconcile`]; this is the glue that drops cross-domain
    /// workspace state and wraps a spawn in `NewLayerShell`.
    fn reconcile_bars(
        &mut self,
        observed: Option<&obayebar_core::hypr::LayerMap>,
    ) -> Task<Message> {
        let expected: std::collections::HashSet<String> =
            self.monitor_geoms.keys().cloned().collect();
        let outcome = self
            .bars
            .reconcile(observed, &expected, std::time::Instant::now());

        let mut tasks: Vec<Task<Message>> =
            outcome.close_ids.into_iter().map(close_window).collect();

        for monitor in &outcome.drop_state_for {
            self.drop_bar_state(monitor);
        }
        if let Some((id, settings)) = outcome.spawn {
            tasks.push(Task::done(Message::NewLayerShell { settings, id }));
        }
        if outcome.needs_verify {
            tasks.push(self.verify_bars_soon());
        }

        Task::batch(tasks)
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
        if self.launcher_window == Some(id) {
            // Dropped rather than reopened: unlike the notification popup,
            // nothing is waiting to be shown, and a launcher that came back on
            // its own after the compositor took it away would be a surprise.
            self.launcher_window = None;
            return Task::none();
        }
        if let Some(kind) = self.forget_panel_window(id) {
            if let Some(setter) = kind.signal_setter() {
                setter(false);
            }
            return Task::none();
        }

        if let Some(record) = self.bars.close_landed(id) {
            // Nothing else to do: the monitor it used to be on was uncovered
            // the moment we asked.
            log::info!("bars: {} closed as requested", record.namespace);
            return Task::none();
        }

        // Unknown ids are surfaces whose tracking we already cleared; they must
        // stay no-ops. Treating one as a bar is what let a panel or popup close
        // masquerade as "the bar died" and trigger a spurious respawn.
        if let Some(record) = self.bars.bar_closed_by_compositor(id) {
            log::info!(
                "bars: {} on {} was closed by the compositor",
                record.namespace,
                record.monitor
            );
            if !self.bars.has_bar_on(&record.monitor) {
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
        if Some(id) == self.launcher_window {
            return self.launcher.view().map(Message::Launcher);
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
        let mut is_animating = self.ws_spring.values().any(SpringState::is_animating);

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
            Subscription::run(control::stream).map(Message::Control),
            // Scans once at startup and then only when the application
            // directories actually change, so the launcher never has a list to
            // rebuild when it opens.
            Subscription::run(launcher::watch::index_stream).map(Message::LauncherIndex),
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

        if let Some(media) = &self.media {
            subs.push(
                Subscription::run(services::media::stream)
                    .map(|players| Message::Media(media::Message::Players(players))),
            );
            is_animating |= self.media_panel_open() && media.animating();
        }

        if is_animating {
            subs.push(
                iced::time::every(std::time::Duration::from_millis(16)).map(|_| Message::AnimTick),
            );
        }

        // Only while the launcher is up: this is the one raw-event feed in the
        // bar, and it fires for every surface.
        if self.launcher_window.is_some() {
            subs.push(iced::event::listen_with(launcher_event));
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
        {
            let audio = Arc::make_mut(&mut self.audio);
            audio.volume = volume;
            audio.icon_name = services::audio::volume_icon(volume, audio.muted);
        }
        services::audio::send_command(AudioCommand::Volume(volume));
        Task::none()
    }

    fn close_all_panels(&mut self) -> Task<Message> {
        self.panel_pointer.clear();
        // Iterate the panels we actually have rather than a hand-maintained
        // `PanelKind::ALL`. That array was the one dispatch table the compiler
        // did not check, and a kind missing from it closed its window while
        // never getting `setter(false)` — leaving its service polling at panel
        // cadence forever. A signal can only be set once its panel has been
        // opened, so this map is exactly the right set.
        let mut tasks = Vec::new();
        for (kind, panel) in &mut self.panels {
            if let Some(setter) = kind.signal_setter() {
                setter(false);
            }
            tasks.push(panel.close());
        }
        Task::batch(tasks)
    }

    /// Compute the content height of `kind`'s panel from current state: it
    /// adapts to dynamic content (sink count, AP count, paired/nearby split, …).
    fn panel_height(&self, kind: PanelKind) -> u32 {
        match kind {
            PanelKind::Audio => style::audio_panel_height(self.audio.sinks.len()),
            PanelKind::Network => {
                let ap_count = self
                    .network
                    .access_points
                    .len()
                    .clamp(1, style::PANEL_MAX_VISIBLE_ROWS);
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
                    .clamp(1, style::PANEL_MAX_VISIBLE_ROWS);
                let nearby = self
                    .bluetooth
                    .devices
                    .iter()
                    .filter(|d| !d.paired)
                    .count()
                    .min(style::PANEL_MAX_VISIBLE_ROWS);
                style::bluetooth_panel_height(
                    paired,
                    nearby,
                    self.bluetooth.powered,
                    self.bluetooth.discovering,
                )
            }
            PanelKind::Sysinfo => style::sysinfo_panel_height(),
            PanelKind::Gitlab => style::GITLAB_PANEL_HEIGHT,
            PanelKind::Media => u32::from(style::MEDIA_PANEL_HEIGHT),
            PanelKind::Calendar => style::calendar_panel_height(),
        }
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
            PanelKind::Media => self
                .media
                .as_ref()
                .map_or_else(|| iced::widget::Space::new().into(), bar::media_panel::view),
            PanelKind::Calendar => bar::calendar_panel::view(&self.time, self.calendar),
        }
    }

    /// Handle a media message. Only a running media module receives any.
    ///
    /// A snapshot that hides the bar entry while the panel is open does not
    /// close the panel directly: the entry's own pointer leave can never
    /// arrive once it is gone, but the panel might still be pinned open by
    /// the pointer sitting over it (say, right after the user paused
    /// playback from inside the panel itself). So this only clears the
    /// trigger half of the pointer state and arms the grace timer, and
    /// [`Message::PanelGraceElapsed`] closes the panel once the pointer is
    /// confirmed to be on neither the trigger nor the panel.
    ///
    /// A hidden entry also drops a hover-open still waiting on it, because
    /// its leave will never arrive.
    fn update_media(&mut self, message: media::Message) -> Task<Message> {
        let Some(media) = self.media.as_mut() else {
            return Task::none();
        };
        match message {
            media::Message::Players(players) => {
                let art = Self::fetch_media_art(media.update(players));
                if media.trigger().is_none() {
                    self.open_intent.left(PanelKind::Media);
                    if self.media_panel_open() {
                        self.panel_pointer.left_trigger(PanelKind::Media);
                        return Task::batch([art, Self::arm_panel_grace()]);
                    }
                }
                art
            }
            media::Message::Art(url, art) => {
                media.art_loaded(url, art);
                Task::none()
            }
            media::Message::Control(action) => {
                if let Some((bus_name, command)) = media.apply(action) {
                    services::media::send(bus_name, command);
                }
                Task::none()
            }
            media::Message::CyclePlayer => Self::fetch_media_art(media.cycle_player()),
        }
    }

    /// Whether the media panel is up.
    fn media_panel_open(&self) -> bool {
        self.panels
            .get(&PanelKind::Media)
            .is_some_and(panel::Panel::is_open)
    }

    /// Fetch the cover the media state asked for, off the UI thread.
    fn fetch_media_art(url: Option<String>) -> Task<Message> {
        url.map_or_else(Task::none, |url| {
            Task::perform(services::media_art::load(url.clone()), move |art| {
                Message::Media(media::Message::Art(url, art))
            })
        })
    }

    /// Schedule a dismissal check after `PANEL_GRACE`.
    ///
    /// A leave never closes directly. The pointer crossing from a bar icon into
    /// its panel produces a leave and an enter, and closing on the leave would
    /// destroy the panel the user is moving into; the grace lets the enter land
    /// first. The check itself is idempotent, so overlapping graces are fine.
    fn arm_panel_grace() -> Task<Message> {
        Task::perform(tokio::time::sleep(PANEL_GRACE), |()| {
            Message::PanelGraceElapsed
        })
    }

    /// Wake up with `ticket` once the pointer has rested on a trigger for the
    /// configured open delay.
    fn arm_open_delay(ticket: panel::Ticket) -> Task<Message> {
        Task::perform(
            tokio::time::sleep(config::resolved().panel_open_delay()),
            move |()| Message::PanelOpenDelayElapsed(ticket),
        )
    }

    /// A hover asked to open `request`'s panel: at once if another panel is
    /// already up, otherwise once the open delay elapses.
    fn hover_panel(&mut self, request: panel::OpenRequest) -> Task<Message> {
        let a_panel_is_open = self.panels.values().any(panel::Panel::is_open);
        match self.open_intent.hover(request, a_panel_is_open) {
            panel::Hovered::OpenNow(request) => self.open_panel(request),
            panel::Hovered::Wait(ticket) => Self::arm_open_delay(ticket),
        }
    }

    /// `ticket`'s open delay elapsed: open its panel if that hover is still
    /// the pending one.
    fn settle_open_delay(&mut self, ticket: panel::Ticket) -> Task<Message> {
        self.open_intent
            .settle(ticket)
            .map_or_else(Task::none, |request| self.open_panel(request))
    }

    /// Open `kind`'s popup, replacing whichever panel is currently shown.
    fn open_panel(
        &mut self,
        panel::OpenRequest {
            kind,
            monitor,
            spot,
        }: panel::OpenRequest,
    ) -> Task<Message> {
        self.open_intent.cancel();
        // A bar with no monitor is not a state we can place a panel from, and
        // guessing an output is what the `LastOutput` fallback used to do.
        // Every bar surface is now tracked with its monitor, so this only fires
        // if the bar was closed between render and hover.
        let Some(monitor) = monitor else {
            log::warn!("panels: not opening {kind:?}, its bar has no known monitor");
            return Task::none();
        };

        // The pointer is on this trigger by definition — recording it here is
        // what stops a leave dispatched in the same batch (crossing from an
        // open panel back onto a bar icon) from closing the panel we are about
        // to open.
        self.panel_pointer.entered_trigger(kind);

        // Already showing: nothing to rebuild. Re-hovering the same icon used
        // to destroy and recreate the surface, which the compositor renders as
        // a flicker.
        if self.panels.get(&kind).is_some_and(panel::Panel::is_open) {
            return Task::none();
        }

        let close = self.close_all_panels();
        // `close_all_panels` clears the pointer state, but the pointer really
        // is on this trigger, so restore it.
        self.panel_pointer.entered_trigger(kind);
        let height = self.panel_height(kind);
        reset_panel_view(&mut self.calendar, kind);
        if let Some(setter) = kind.signal_setter() {
            setter(true);
        }
        let open = self
            .panels
            .entry(kind)
            .or_default()
            .open(kind, height, &monitor, spot);
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

    /// Height cap in logical pixels for a popup on `geom`, or the
    /// 1080p-based fallback when no monitor geometry is known yet. The cap
    /// is the `style::NOTIF_POPUP_MAX_FRACTION_NUM`/`_DEN` fraction of the
    /// oriented logical height.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        clippy::as_conversions
    )]
    fn popup_height_cap(geom: Option<&MonitorGeom>) -> u32 {
        const FALLBACK_LOGICAL_H: f32 = 1080.0;
        let num = f32::from(u16::try_from(style::NOTIF_POPUP_MAX_FRACTION_NUM).unwrap_or(2));
        let den = f32::from(u16::try_from(style::NOTIF_POPUP_MAX_FRACTION_DEN).unwrap_or(5));

        let logical_h = geom.map_or(FALLBACK_LOGICAL_H, |g| {
            let scale = if g.scale > 0.0 { g.scale } else { 1.0 };
            let (_, raw) = g.oriented_size();
            raw as f32 / scale
        });

        (logical_h * num / den) as u32
    }

    /// Maximum popup height in logical pixels, for the monitor the popup is
    /// *on*. See [`Self::popup_height_cap`].
    ///
    /// Measuring `focused_monitor` instead of the popup's own monitor was
    /// wrong in both directions: the popup did not live there, and every
    /// focus change re-fitted the surface against a screen it was not on.
    /// With a 4K focused monitor and a 768px host that produced a cap taller
    /// than the screen, and since the popup column has no scrollable the
    /// overflow summary itself fell off-screen.
    fn popup_max_height(&self) -> u32 {
        let geom = self
            .notif_popup_monitor
            .as_deref()
            .or(self.focused_monitor.as_deref())
            .and_then(|name| self.monitor_geoms.get(name));
        Self::popup_height_cap(geom)
    }

    /// Decide how many popup cards fit and how many spill into an overflow
    /// summary entry, using the focused monitor's screen cap.
    fn popup_fit(&self) -> (usize, usize) {
        style::notif_popup_fit(self.popup_notifications.len(), self.popup_max_height())
    }

    /// The monitor an overlay belongs on — the notification popup, the
    /// launcher: the focused one while it is still connected, otherwise any
    /// connected monitor (lowest name, so the choice is stable rather than
    /// flipping on `HashMap` order).
    fn overlay_monitor(&self) -> Option<String> {
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
        let Some(target) = self.overlay_monitor() else {
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

/// View state to drop back to when a panel opens fresh, per kind. Only the
/// calendar carries any: it always opens on the current month, never on
/// wherever a previous visit paged it to.
fn reset_panel_view(calendar: &mut calendar::Pager, kind: PanelKind) {
    if kind == PanelKind::Calendar {
        *calendar = calendar::Pager::default();
    }
}

/// Count connections per type group (preserving insertion order).
fn connection_type_groups(conns: &[services::network::ActiveConnectionInfo]) -> Vec<usize> {
    let mut groups: Vec<(&str, usize)> = Vec::new();
    for ac in conns {
        if let Some(g) = groups.iter_mut().find(|(t, _)| *t == ac.conn_type) {
            g.1 = g.1.saturating_add(1);
        } else {
            groups.push((&ac.conn_type, 1));
        }
    }
    groups.into_iter().map(|(_, c)| c).collect()
}

/// Keyboard and window events, tagged with the surface they happened on.
///
/// A plain `fn` because `listen_with` takes a function pointer, not a closure:
/// the filtering by window id has to happen in `update`, where the launcher's
/// current id is known.
fn launcher_event(event: Event, _status: iced::event::Status, id: window::Id) -> Option<Message> {
    matches!(event, Event::Keyboard(_) | Event::Window(_))
        .then(|| Message::LauncherEvent(id, event))
}

fn theme_fn(_app: &App, _id: window::Id) -> Theme {
    style::m3_theme("obayebar-dark")
}

fn close_window(id: window::Id) -> Task<Message> {
    iced_runtime::task::effect(iced_runtime::Action::Window(
        iced_runtime::window::Action::Close(id),
    ))
}

/// Where the pointer is relative to the panels, which is the whole basis for
/// dismissing one.
///
/// The two locations are tracked separately rather than as a single "hovered"
/// value, because a pointer crossing from a bar icon into its panel produces a
/// leave *and* an enter, and the order they are dispatched in is fixed by
/// window id (messages are drained from a `BTreeMap` keyed on it) — a panel's
/// id is always newer than its bar's. Keeping them apart makes every
/// interleaving give the same answer, so the behaviour does not rest on that
/// ordering holding.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct PanelPointer {
    /// The bar trigger (status icon) the pointer is over.
    on_trigger: Option<PanelKind>,
    /// The panel surface the pointer is over.
    on_panel: Option<PanelKind>,
}

impl PanelPointer {
    const fn entered_trigger(&mut self, kind: PanelKind) {
        self.on_trigger = Some(kind);
    }

    /// A leave only clears the location if it is still the one recorded — a
    /// stale leave for a trigger the pointer has already moved off must not
    /// undo the newer position.
    fn left_trigger(&mut self, kind: PanelKind) {
        if self.on_trigger == Some(kind) {
            self.on_trigger = None;
        }
    }

    const fn entered_panel(&mut self, kind: PanelKind) {
        self.on_panel = Some(kind);
    }

    fn left_panel(&mut self, kind: PanelKind) {
        if self.on_panel == Some(kind) {
            self.on_panel = None;
        }
    }

    const fn clear(&mut self) {
        self.on_trigger = None;
        self.on_panel = None;
    }

    /// True when the pointer is on neither a trigger nor a panel, i.e. the
    /// panels should be dismissed.
    const fn is_away(self) -> bool {
        self.on_trigger.is_none() && self.on_panel.is_none()
    }
}

#[cfg(test)]
mod panel_pointer_tests {
    use super::{PanelKind, PanelPointer};

    #[test]
    fn a_fresh_pointer_is_away() {
        assert!(PanelPointer::default().is_away());
    }

    #[test]
    fn hovering_a_trigger_holds_the_panel_open() {
        let mut p = PanelPointer::default();
        p.entered_trigger(PanelKind::Audio);
        assert!(!p.is_away());
    }

    #[test]
    fn leaving_the_trigger_without_entering_the_panel_dismisses() {
        // The reported stuck-panel bug: moving up the bar past the clock, or
        // off to another monitor, never entered the panel — and the panel's own
        // on_exit was the only dismissal producer, so nothing ever fired.
        let mut p = PanelPointer::default();
        p.entered_trigger(PanelKind::Audio);
        p.left_trigger(PanelKind::Audio);
        assert!(p.is_away(), "must dismiss when the pointer just leaves");
    }

    #[test]
    fn crossing_from_trigger_into_panel_keeps_it_open_either_order() {
        // Order is decided by window id, not by us. Both interleavings must
        // agree, or the panel the user is moving into gets destroyed.
        let mut leave_first = PanelPointer::default();
        leave_first.entered_trigger(PanelKind::Audio);
        leave_first.left_trigger(PanelKind::Audio);
        leave_first.entered_panel(PanelKind::Audio);
        assert!(!leave_first.is_away());

        let mut enter_first = PanelPointer::default();
        enter_first.entered_trigger(PanelKind::Audio);
        enter_first.entered_panel(PanelKind::Audio);
        enter_first.left_trigger(PanelKind::Audio);
        assert!(!enter_first.is_away());

        assert_eq!(leave_first, enter_first);
    }

    #[test]
    fn leaving_the_panel_dismisses() {
        let mut p = PanelPointer::default();
        p.entered_trigger(PanelKind::Audio);
        p.left_trigger(PanelKind::Audio);
        p.entered_panel(PanelKind::Audio);
        p.left_panel(PanelKind::Audio);
        assert!(p.is_away());
    }

    #[test]
    fn crossing_from_a_panel_onto_another_trigger_keeps_a_panel_open() {
        // Audio panel open, pointer moves onto the Network icon. The bar
        // publishes the open before the panel publishes its leave, so the leave
        // must not dismiss the panel that was just requested.
        let mut p = PanelPointer::default();
        p.entered_panel(PanelKind::Audio);
        p.entered_trigger(PanelKind::Network);
        p.left_panel(PanelKind::Audio);
        assert!(!p.is_away());
        assert_eq!(p.on_trigger, Some(PanelKind::Network));
    }

    #[test]
    fn a_stale_leave_does_not_clear_a_newer_position() {
        // Icon-to-icon: the leave for the old icon must not undo the enter for
        // the new one, whichever order they arrive in.
        let mut p = PanelPointer::default();
        p.entered_trigger(PanelKind::Audio);
        p.entered_trigger(PanelKind::Network);
        p.left_trigger(PanelKind::Audio);
        assert!(!p.is_away());
        assert_eq!(p.on_trigger, Some(PanelKind::Network));
    }

    #[test]
    fn a_stale_panel_leave_does_not_clear_a_newer_panel() {
        let mut p = PanelPointer::default();
        p.entered_panel(PanelKind::Audio);
        p.entered_panel(PanelKind::Network);
        p.left_panel(PanelKind::Audio);
        assert_eq!(p.on_panel, Some(PanelKind::Network));
        assert!(!p.is_away());
    }

    #[test]
    fn clearing_forgets_both_locations() {
        // `close_all_panels` runs on monitor changes and explicit closes; the
        // pointer bookkeeping must not outlive the surfaces it described.
        let mut p = PanelPointer::default();
        p.entered_trigger(PanelKind::Audio);
        p.entered_panel(PanelKind::Audio);
        p.clear();
        assert!(p.is_away());
    }
}

#[cfg(test)]
mod reset_panel_view_tests {
    use super::{reset_panel_view, PanelKind};
    use crate::calendar::{Pager, Step};

    #[test]
    fn opening_the_calendar_resets_its_pager() {
        let mut pager = Pager::default();
        pager.page(Step::Next);
        reset_panel_view(&mut pager, PanelKind::Calendar);
        assert_eq!(pager, Pager::default());
    }

    #[test]
    fn opening_another_panel_leaves_the_pager_alone() {
        let mut pager = Pager::default();
        pager.page(Step::Next);
        let before = pager;
        reset_panel_view(&mut pager, PanelKind::Audio);
        assert_eq!(pager, before);
    }
}

#[cfg(test)]
mod popup_height_cap_tests {
    use super::App;
    use obayebar_core::hypr::MonitorGeom;

    fn geom(width: u32, height: u32, scale: f32, transform: i32) -> MonitorGeom {
        MonitorGeom {
            width,
            height,
            scale,
            transform,
        }
    }

    #[test]
    fn falls_back_to_a_1080p_based_cap_without_geometry() {
        assert_eq!(App::popup_height_cap(None), 432);
    }

    #[test]
    fn upright_caps_on_the_logical_height() {
        let g = geom(2560, 1440, 1.0, 0);
        assert_eq!(App::popup_height_cap(Some(&g)), 576);
    }

    #[test]
    fn rotated_caps_on_the_logical_width() {
        let g = geom(2560, 1440, 1.0, 1);
        assert_eq!(App::popup_height_cap(Some(&g)), 1024);
    }

    #[test]
    fn scale_shrinks_the_logical_size_before_capping() {
        let g = geom(2560, 1440, 2.0, 0);
        assert_eq!(App::popup_height_cap(Some(&g)), 288);
    }

    #[test]
    fn a_non_positive_scale_is_treated_as_unscaled() {
        let g = geom(2560, 1440, 0.0, 0);
        assert_eq!(App::popup_height_cap(Some(&g)), 576);
    }
}

#[cfg(test)]
mod usage_tests {
    use super::USAGE;

    #[test]
    fn matches_the_readme_verbatim() {
        let readme = include_str!("../../../README.md");
        assert!(
            readme.contains(USAGE),
            "USAGE is not pasted into the README verbatim"
        );
    }
}

#[cfg(test)]
mod connection_type_groups_tests {
    use super::connection_type_groups;
    use crate::services::network::ActiveConnectionInfo;

    fn conn(conn_type: &str) -> ActiveConnectionInfo {
        ActiveConnectionInfo {
            name: conn_type.to_string(),
            conn_type: conn_type.to_string(),
            icon_name: "icon",
        }
    }

    #[test]
    fn empty_input_has_no_groups() {
        assert_eq!(connection_type_groups(&[]), Vec::<usize>::new());
    }

    #[test]
    fn counts_each_type_preserving_first_seen_order() {
        let conns = [conn("vpn"), conn("ethernet"), conn("vpn")];
        assert_eq!(connection_type_groups(&conns), vec![2, 1]);
    }

    #[test]
    fn single_type_repeated_counts_all_of_them() {
        let conns: Vec<_> = std::iter::repeat_with(|| conn("wireguard"))
            .take(5)
            .collect();
        assert_eq!(connection_type_groups(&conns), vec![5]);
    }
}
