use iced::window;
use iced_layershell::reexport::{
    Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings, OutputOption,
};

use crate::Message;
use obayebar::style;

#[derive(Debug)]
pub struct Panel {
    id: Option<window::Id>,
    open: bool,
}

impl Panel {
    pub const fn new() -> Self {
        Self {
            id: None,
            open: false,
        }
    }

    pub fn is_window(&self, id: window::Id) -> bool {
        self.id == Some(id)
    }

    pub fn open(
        &mut self,
        width: u32,
        height: u32,
        monitor: Option<String>,
    ) -> iced::Task<Message> {
        self.open_with(width, height, monitor, KeyboardInteractivity::None)
    }

    /// Same as [`open`], but keeps keyboard input flowing into the popup.
    /// Use for popups that contain a text input.
    pub fn open_keyboard(
        &mut self,
        width: u32,
        height: u32,
        monitor: Option<String>,
    ) -> iced::Task<Message> {
        self.open_with(width, height, monitor, KeyboardInteractivity::OnDemand)
    }

    fn open_with(
        &mut self,
        width: u32,
        height: u32,
        monitor: Option<String>,
        keyboard_interactivity: KeyboardInteractivity,
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
                keyboard_interactivity,
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
}
