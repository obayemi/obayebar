use crate::services::hyprland::WorkspaceInfo;
use crate::style;
use crate::Message;
use iced::mouse;
use iced::widget::canvas::{self, Cache, Frame, Geometry, Path};
use iced::widget::container;
use iced::{alignment, Element, Length, Point, Rectangle, Renderer, Size, Theme};

/// Spring-based animation state for the workspace indicator
#[derive(Debug, Clone)]
pub struct SpringState {
    pub position: f32,
    velocity: f32,
    pub target: f32,
}

// Spring parameters – critically-damped feel
const STIFFNESS: f32 = 300.0;
const DAMPING: f32 = 30.0;

impl Default for SpringState {
    fn default() -> Self {
        Self {
            position: 0.0,
            velocity: 0.0,
            target: 0.0,
        }
    }
}

impl SpringState {
    pub const fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Advance by `dt` seconds. Returns `true` while still moving.
    pub fn tick(&mut self, dt: f32) -> bool {
        let displacement = self.position - self.target;
        let accel = DAMPING.mul_add(-self.velocity, -STIFFNESS * displacement);
        self.velocity = accel.mul_add(dt, self.velocity);
        self.position = self.velocity.mul_add(dt, self.position);

        let settled = displacement.abs() < 0.001 && self.velocity.abs() < 0.01;
        if settled {
            self.position = self.target;
            self.velocity = 0.0;
        }
        !settled
    }

    pub fn is_animating(&self) -> bool {
        (self.position - self.target).abs() > 0.001 || self.velocity.abs() > 0.01
    }

    /// Snap position to target without animation
    pub const fn snap(&mut self, pos: f32) {
        self.position = pos;
        self.velocity = 0.0;
        self.target = pos;
    }
}

const fn cell_size() -> f32 {
    style::BAR_INNER_WIDTH - style::PADDING_SMALL * 2.0
}

const fn cell_spacing() -> f32 {
    style::SPACING_SMALL / 2.0
}

struct WorkspaceCanvas<'a> {
    workspaces: Vec<(i32, bool, bool)>, // (id, is_active, is_occupied)
    indicator_pos: f32,
    cache: &'a Cache,
}

impl canvas::Program<Message> for WorkspaceCanvas<'_> {
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
                    #[allow(clippy::cast_precision_loss)]
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
                        size: iced::Pixels(style::FONT_SIZE_LARGER),
                        font: iced::Font {
                            weight: iced::font::Weight::Bold,
                            ..iced::Font::MONOSPACE
                        },
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
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if matches!(
            event,
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
        ) {
            if let Some(pos) = cursor.position_in(bounds) {
                let step = cell_size() + cell_spacing();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let index = (pos.y / step) as usize;

                if let Some(&(id, _, _)) = self.workspaces.get(index) {
                    return Some(
                        canvas::Action::publish(Message::WorkspaceClick(id)).and_capture(),
                    );
                }
            }
        }
        None
    }
}

pub fn view<'a>(
    workspaces: &[&WorkspaceInfo],
    active: i32,
    spring: &SpringState,
    cache: &'a Cache,
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
    let canvas_height = (count as f32).mul_add(size + spacing, -spacing);

    let canvas_widget = iced::widget::canvas(WorkspaceCanvas {
        workspaces: ws_data,
        indicator_pos: spring.position,
        cache,
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
