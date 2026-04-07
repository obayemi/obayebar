use crate::services::hyprland::WorkspaceInfo;
use crate::style;
use crate::Message;
use iced::mouse;
use iced::widget::canvas::{self, Cache, Frame, Geometry, Path};
use iced::widget::{container, Action};
use iced::{alignment, Element, Event, Length, Point, Rectangle, Renderer, Size, Theme};

/// Animation state for the workspace active indicator
#[derive(Debug, Clone)]
pub struct AnimState {
    /// Current animated index position (fractional for smooth movement)
    pub current_pos: f32,
    /// Target index position
    pub target_pos: f32,
    /// Whether animation is in progress
    pub animating: bool,
}

impl Default for AnimState {
    fn default() -> Self {
        Self {
            current_pos: 0.0,
            target_pos: 0.0,
            animating: false,
        }
    }
}

impl AnimState {
    /// Update target to a new workspace index, starting animation
    pub fn set_target(&mut self, index: f32) {
        if (self.target_pos - index).abs() > f32::EPSILON {
            self.target_pos = index;
            self.animating = true;
        }
    }

    /// Advance the animation by one frame. Returns true if still animating.
    pub fn tick(&mut self) -> bool {
        if !self.animating {
            return false;
        }

        let diff = self.target_pos - self.current_pos;
        if diff.abs() < 0.01 {
            self.current_pos = self.target_pos;
            self.animating = false;
            return false;
        }

        // Ease-out: move 15% of remaining distance per frame (~60fps, ~200ms feel)
        self.current_pos += diff * 0.15;
        true
    }
}

struct WorkspaceCanvas {
    workspaces: Vec<(i32, bool, bool)>, // (id, is_active, is_occupied)
    indicator_pos: f32,
    cache: Cache,
}

const fn cell_size() -> f32 {
    style::PADDING_SMALL.mul_add(-2.0, style::BAR_INNER_WIDTH)
}

fn cell_spacing() -> f32 {
    style::SPACING_SMALL / 2.0
}

impl canvas::Program<Message> for WorkspaceCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let geometry = self
            .cache
            .draw(renderer, bounds.size(), |frame: &mut Frame| {
                let size = cell_size();
                let spacing = cell_spacing();

                if self.workspaces.is_empty() {
                    return;
                }

                // Draw the sliding indicator pill behind the active workspace
                let indicator_y = self.indicator_pos * (size + spacing);
                let indicator_rect = Path::rounded_rectangle(
                    Point::new((bounds.width - size) / 2.0, indicator_y),
                    Size::new(size, size),
                    style::ROUNDING_FULL.into(),
                );
                frame.fill(&indicator_rect, style::with_alpha(style::M3_PRIMARY, 0.15));

                // Draw workspace labels
                for (i, &(id, is_active, is_occupied)) in self.workspaces.iter().enumerate() {
                    #[allow(clippy::cast_precision_loss)] // workspace count is tiny
                    let y = (i as f32) * (size + spacing);

                    let color = if is_active {
                        style::M3_PRIMARY
                    } else if is_occupied {
                        style::M3_ON_SURFACE
                    } else {
                        style::M3_OUTLINE_VARIANT
                    };

                    let label = canvas::Text {
                        content: id.to_string(),
                        position: Point::new(bounds.width / 2.0, y + size / 2.0),
                        color,
                        size: iced::Pixels(style::FONT_SIZE_NORMAL),
                        font: iced::Font::MONOSPACE,
                        align_x: iced::Alignment::Center.into(),
                        align_y: alignment::Vertical::Center,
                        ..canvas::Text::default()
                    };
                    frame.fill_text(label);
                }
            });

        vec![geometry]
    }

    fn mouse_interaction(
        &self,
        _state: &(),
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }

    fn update(
        &self,
        _state: &mut (),
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<Message>> {
        if matches!(
            event,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
        ) {
            if let Some(pos) = cursor.position_in(bounds) {
                let step = cell_size() + cell_spacing();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let index = (pos.y / step) as usize;

                if let Some(&(id, _, _)) = self.workspaces.get(index) {
                    return Some(Action::publish(Message::WorkspaceClick(id)).and_capture());
                }
            }
        }
        None
    }
}

pub fn view<'a>(
    workspaces: &[&WorkspaceInfo],
    active: i32,
    anim_state: &AnimState,
) -> Element<'a, Message> {
    let mut sorted: Vec<&WorkspaceInfo> = workspaces
        .iter()
        .filter(|w| w.id > 0 && !w.name.starts_with("special:"))
        .copied()
        .collect();
    sorted.sort_by_key(|w| w.id);

    let ws_data: Vec<(i32, bool, bool)> = sorted
        .iter()
        .map(|w| (w.id, w.id == active, w.windows > 0))
        .collect();

    let size = cell_size();
    let spacing = cell_spacing();
    let count = ws_data.len().max(1);
    #[allow(clippy::cast_precision_loss)]
    #[allow(clippy::cast_precision_loss)] // workspace count is tiny
    let canvas_height = (count as f32).mul_add(size + spacing, -spacing);

    let canvas_widget = iced::widget::canvas(WorkspaceCanvas {
        workspaces: ws_data,
        indicator_pos: anim_state.current_pos,
        cache: Cache::new(),
    })
    .width(style::BAR_INNER_WIDTH)
    .height(canvas_height);

    container(canvas_widget)
        .padding(style::PADDING_SMALL)
        .width(Length::Fill)
        .align_x(iced::Alignment::Center)
        .style(style::pill_container)
        .into()
}
