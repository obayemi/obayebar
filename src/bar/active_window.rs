use crate::services::hyprland::WindowInfo;
use crate::style;
use crate::Message;
use iced::mouse;
use iced::widget::canvas::{self, Cache, Frame, Geometry, Text};
use iced::{alignment, Element, Length, Rectangle, Renderer, Theme};

struct ActiveWindowCanvas {
    title: String,
    cache: Cache,
}

impl canvas::Program<Message> for ActiveWindowCanvas {
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
                if self.title.is_empty() {
                    return;
                }

                frame.with_save(|frame| {
                    frame.translate(iced::Vector::new(bounds.width, bounds.height));
                    frame.rotate(-std::f32::consts::FRAC_PI_2);

                    // Single line, no wrapping — title is pre-truncated in view()
                    let rotated_text = Text {
                        content: self.title.clone(),
                        position: iced::Point::new(bounds.height / 2.0, -bounds.width / 2.0),
                        color: style::M3_PRIMARY,
                        size: iced::Pixels(style::FONT_SIZE_LARGE),
                        font: iced::Font::MONOSPACE,
                        align_x: iced::Alignment::Center.into(),
                        align_y: alignment::Vertical::Center,
                        ..Text::default()
                    };
                    frame.fill_text(rotated_text);
                });
            });

        vec![geometry]
    }
}

/// Truncate a string to fit approximately within `max_chars`, adding ellipsis
fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}\u{2026}")
}

pub fn view(window: Option<&WindowInfo>) -> Element<'_, Message> {
    let title = window.map_or_else(
        || "Desktop".into(),
        |w| {
            // Extract last segment after dash separators (compact mode)
            let parts: Vec<&str> = w.title.split(&['\u{2013}', '\u{2014}', '-'][..]).collect();
            let raw = parts
                .last()
                .map_or_else(|| w.title.clone(), |s| s.trim().to_string());
            // Truncate to ~20 chars to keep it single-line in the bar
            truncate_with_ellipsis(&raw, 20)
        },
    );

    iced::widget::canvas(ActiveWindowCanvas {
        title,
        cache: Cache::new(),
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}
