use iced::window;
use iced_layershell::reexport::{
    Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings, OutputOption,
};

use crate::services;
use crate::Message;
use obayebar::style;

/// One enum variant per settings panel surface, used as the key into
/// `App::panels` and as the discriminator for `Message::PanelOpen`.
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub enum PanelKind {
    Audio,
    Network,
    Battery,
    Bluetooth,
    Sysinfo,
    Gitlab,
}

impl PanelKind {
    /// Iterate every panel kind. Order is stable so close/open behaviour stays
    /// deterministic; iteration is rare enough that the cost is irrelevant.
    pub const ALL: [Self; 6] = [
        Self::Audio,
        Self::Network,
        Self::Battery,
        Self::Bluetooth,
        Self::Sysinfo,
        Self::Gitlab,
    ];

    /// Fixed surface width for this panel kind. Heights are state-dependent
    /// and computed on the fly by the caller.
    pub const fn width(self) -> u32 {
        match self {
            Self::Audio => style::AUDIO_PANEL_WIDTH,
            Self::Network => style::NETWORK_PANEL_WIDTH,
            Self::Battery => style::BATTERY_PANEL_WIDTH,
            Self::Bluetooth => style::BLUETOOTH_PANEL_WIDTH,
            Self::Sysinfo => style::SYSINFO_PANEL_WIDTH,
            Self::Gitlab => style::GITLAB_PANEL_WIDTH,
        }
    }

    /// `Some` when this kind drives a service-side `PanelSignal` that should
    /// flip on open/close so the backing service can switch refresh cadence
    /// (network rescan, bluetooth discovery hint, sysinfo polling, gitlab
    /// rate). `None` for kinds whose service runs at a single cadence.
    pub fn signal_setter(self) -> Option<fn(bool)> {
        match self {
            Self::Network => Some(services::network::set_panel_open),
            Self::Bluetooth => Some(services::bluetooth::set_panel_open),
            Self::Sysinfo => Some(services::sysinfo::set_panel_open),
            Self::Gitlab => Some(services::gitlab::set_panel_open),
            Self::Audio | Self::Battery => None,
        }
    }
}

#[derive(Debug, Default)]
pub struct Panel {
    id: Option<window::Id>,
    open: bool,
}

impl Panel {
    pub fn is_window(&self, id: window::Id) -> bool {
        self.id == Some(id)
    }

    pub fn open(
        &mut self,
        width: u32,
        height: u32,
        monitor: Option<String>,
    ) -> iced::Task<Message> {
        if self.open {
            return iced::Task::none();
        }
        self.open = true;
        let id = window::Id::unique();
        self.id = Some(id);
        let output_option = monitor.map_or(OutputOption::LastOutput, OutputOption::OutputName);
        let gap = style::PANEL_GAP_PX;
        iced::Task::done(Message::NewLayerShell {
            settings: NewLayerShellSettings {
                anchor: Anchor::Left | Anchor::Bottom,
                layer: Layer::Overlay,
                exclusive_zone: Some(-1),
                size: Some((width.saturating_add(gap), height.saturating_add(gap))),
                margin: Some((0, 0, 0, style::BAR_WIDTH.cast_signed())),
                keyboard_interactivity: KeyboardInteractivity::None,
                output_option,
                ..NewLayerShellSettings::default()
            },
            id,
        })
    }

    pub fn close(&mut self) -> iced::Task<Message> {
        self.open = false;
        self.id
            .take()
            .map_or_else(iced::Task::none, super::close_window)
    }

    /// Drop the panel's tracked window id without dispatching a Close action.
    /// Returns true if `id` matched this panel — the caller should run its
    /// own state cleanup as if `close()` had been invoked.
    pub fn forget_if(&mut self, id: window::Id) -> bool {
        if self.id == Some(id) {
            self.id = None;
            self.open = false;
            true
        } else {
            false
        }
    }
}
