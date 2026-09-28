use iced::window;
use iced_layershell::reexport::{
    Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings, OutputOption,
};

use crate::services;
use crate::Message;
use obayebar::style;

mod intent;
mod placement;

pub use intent::{Hovered, OpenIntent, OpenRequest, Ticket};

pub use placement::TriggerSpot;

/// One enum variant per popup panel surface, used as the key into
/// `App::panels` and as the discriminator of an [`OpenRequest`].
#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub enum PanelKind {
    Audio,
    Network,
    Battery,
    Bluetooth,
    Sysinfo,
    Gitlab,
    Media,
    Calendar,
}

impl PanelKind {
    /// Layer-shell namespace for this kind's surface.
    pub fn namespace(self) -> String {
        let suffix = match self {
            Self::Audio => "audio",
            Self::Network => "network",
            Self::Battery => "battery",
            Self::Bluetooth => "bluetooth",
            Self::Sysinfo => "sysinfo",
            Self::Gitlab => "gitlab",
            Self::Media => "media",
            Self::Calendar => "calendar",
        };
        format!("obayebar-panel-{suffix}")
    }

    /// `Some` when this kind drives a service-side `PanelSignal` that should
    /// flip on open/close so the backing service can switch refresh cadence
    /// (network rescan, bluetooth discovery hint, sysinfo polling, gitlab
    /// rate, media position resampling). `None` for kinds with no backing
    /// service, or whose service runs at a single cadence.
    pub fn signal_setter(self) -> Option<fn(bool)> {
        match self {
            Self::Network => Some(services::network::set_panel_open),
            Self::Bluetooth => Some(services::bluetooth::set_panel_open),
            Self::Sysinfo => Some(services::sysinfo::set_panel_open),
            Self::Gitlab => Some(services::gitlab::set_panel_open),
            Self::Media => Some(services::media::set_panel_open),
            Self::Audio | Self::Battery | Self::Calendar => None,
        }
    }
}

/// An open panel's layer-shell window, its last-sized content height, and the
/// trigger it stays centred on.
#[derive(Debug)]
struct Surface {
    id: window::Id,
    height: u32,
    spot: TriggerSpot,
}

#[derive(Debug, Default)]
pub struct Panel {
    surface: Option<Surface>,
}

impl Panel {
    /// Layer-shell surface size for a panel with content `height`, every panel
    /// sharing [`style::PANEL_WIDTH`].
    ///
    /// The gap between bar and panel is part of the surface so the compositor
    /// includes it in the input region, and the pointer crossing it never
    /// leaves the panel. `open` and `resize` must agree on it, which is why it
    /// lives here rather than being spelled out at each site.
    const fn surface_size(height: u32) -> (u32, u32) {
        (
            style::PANEL_WIDTH.saturating_add(style::PANEL_GAP_PX),
            height,
        )
    }

    /// Whether this panel currently has a surface.
    pub const fn is_open(&self) -> bool {
        self.surface.is_some()
    }

    /// Resize the open surface to fit content of `height`.
    ///
    /// Panels used to be sized exactly once, at creation: `Message::SizeChange`
    /// had a single emitter (the notification popup), and every service-update
    /// arm returned `Task::none()`. A panel opened before its payload arrived —
    /// which was guaranteed, since the services withheld their lists until the
    /// panel-open signal flipped — kept the wrong height for its whole
    /// lifetime.
    ///
    /// The surface moves with its size so it stays centred on its trigger.
    pub fn resize(&mut self, height: u32) -> iced::Task<Message> {
        let Some(surface) = &mut self.surface else {
            return iced::Task::none();
        };
        if surface.height == height {
            return iced::Task::none();
        }
        surface.height = height;
        iced::Task::batch([
            iced::Task::done(Message::SizeChange {
                id: surface.id,
                size: Self::surface_size(height),
            }),
            iced::Task::done(Message::MarginChange {
                id: surface.id,
                margin: surface.spot.margin(height),
            }),
        ])
    }

    pub fn is_window(&self, id: window::Id) -> bool {
        self.surface.as_ref().is_some_and(|s| s.id == id)
    }

    /// Open this panel on `monitor`, beside the bar and centred on `spot`.
    ///
    /// `monitor` is required rather than optional. The old `None` branch used
    /// `OutputOption::LastOutput`, which resolves through `last_wloutput` —
    /// only ever advanced by a pointer *button* press, a keyboard enter or a
    /// touch. Bars are `KeyboardInteractivity::None` and nothing ever sends
    /// `ForgetLastOutput`, so it was a sticky arbitrary output: the panel could
    /// open on a screen the pointer was nowhere near. Making the monitor
    /// mandatory removes that state from the type.
    pub fn open(
        &mut self,
        kind: PanelKind,
        height: u32,
        monitor: &str,
        spot: TriggerSpot,
    ) -> iced::Task<Message> {
        if self.surface.is_some() {
            // Unreachable: `open_panel` short-circuits when this kind is
            // already showing. Logged rather than silently ignored so a
            // regression here is visible instead of leaking a window id.
            log::error!("panels: open() called for an already-open panel");
            return iced::Task::none();
        }
        let id = window::Id::unique();
        self.surface = Some(Surface { id, height, spot });
        iced::Task::done(Message::NewLayerShell {
            settings: NewLayerShellSettings {
                anchor: Anchor::Left | Anchor::Top,
                layer: Layer::Overlay,
                exclusive_zone: Some(-1),
                size: Some(Self::surface_size(height)),
                margin: Some(spot.margin(height)),
                keyboard_interactivity: KeyboardInteractivity::None,
                output_option: OutputOption::OutputName(monitor.to_string()),
                // Per-kind namespace so `j/layers` can tell a panel from a bar
                // (and from another panel), and so a Hyprland `layerrule` can
                // target one without catching all of them.
                namespace: Some(kind.namespace()),
                ..NewLayerShellSettings::default()
            },
            id,
        })
    }

    pub fn close(&mut self) -> iced::Task<Message> {
        self.surface
            .take()
            .map_or_else(iced::Task::none, |s| super::close_window(s.id))
    }

    /// Drop the panel's tracked window id without dispatching a Close action.
    /// Returns true if `id` matched this panel — the caller should run its
    /// own state cleanup as if `close()` had been invoked.
    pub fn forget_if(&mut self, id: window::Id) -> bool {
        if self.is_window(id) {
            self.surface = None;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use iced::Rectangle;

    use super::*;

    #[test]
    fn surface_adds_the_gap_on_the_bar_side_only() {
        assert_eq!(Panel::surface_size(184), (368, 184));
    }

    fn open_id(panel: &Panel) -> window::Id {
        let Some(surface) = &panel.surface else {
            unreachable!("panel should be open");
        };
        surface.id
    }

    fn spot() -> TriggerSpot {
        TriggerSpot::new(
            Rectangle {
                x: 0.0,
                y: 0.0,
                width: 54.0,
                height: 20.0,
            },
            Rectangle {
                x: 0.0,
                y: 0.0,
                width: 54.0,
                height: 1080.0,
            },
        )
    }

    #[test]
    fn a_fresh_panel_is_closed() {
        let panel = Panel::default();
        assert!(!panel.is_open());
        assert!(!panel.is_window(window::Id::unique()));
    }

    #[test]
    fn opening_marks_the_panel_open_and_tracks_its_window() {
        let mut panel = Panel::default();
        let _ = panel.open(PanelKind::Audio, 50, "DP-1", spot());
        assert!(panel.is_open());
        assert!(panel.is_window(open_id(&panel)));
    }

    #[test]
    fn opening_an_already_open_panel_is_a_noop() {
        let mut panel = Panel::default();
        let _ = panel.open(PanelKind::Audio, 50, "DP-1", spot());
        let id = open_id(&panel);
        let _ = panel.open(PanelKind::Audio, 60, "DP-2", spot());
        assert_eq!(panel.surface.as_ref().map(|s| s.id), Some(id));
        assert_eq!(panel.surface.as_ref().map(|s| s.height), Some(50));
    }

    #[test]
    fn closing_clears_the_panel() {
        let mut panel = Panel::default();
        let _ = panel.open(PanelKind::Audio, 50, "DP-1", spot());
        let id = open_id(&panel);
        let _ = panel.close();
        assert!(!panel.is_open());
        assert!(!panel.is_window(id));
    }

    #[test]
    fn forget_if_matching_id_closes_the_panel() {
        let mut panel = Panel::default();
        let _ = panel.open(PanelKind::Audio, 50, "DP-1", spot());
        let id = open_id(&panel);
        assert!(panel.forget_if(id));
        assert!(!panel.is_open());
    }

    #[test]
    fn forget_if_a_different_id_does_nothing() {
        let mut panel = Panel::default();
        let _ = panel.open(PanelKind::Audio, 50, "DP-1", spot());
        assert!(!panel.forget_if(window::Id::unique()));
        assert!(panel.is_open());
    }

    #[test]
    fn resize_before_open_is_a_noop() {
        let mut panel = Panel::default();
        let _ = panel.resize(10);
        assert!(!panel.is_open());
    }

    #[test]
    fn resize_updates_the_tracked_size() {
        let mut panel = Panel::default();
        let _ = panel.open(PanelKind::Audio, 50, "DP-1", spot());
        let _ = panel.resize(60);
        assert_eq!(panel.surface.as_ref().map(|s| s.height), Some(60));
    }
}
