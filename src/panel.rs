use iced::window;
use iced_layershell::reexport::{
    Anchor, KeyboardInteractivity, Layer, NewLayerShellSettings, OutputOption,
};

use crate::style;
use crate::Message;

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
                size: Some((width + gap, height + gap)),
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
        if let Some(id) = self.id.take() {
            super::close_window(id)
        } else {
            iced::Task::none()
        }
    }
}
