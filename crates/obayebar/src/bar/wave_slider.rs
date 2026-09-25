//! The Android-style progress slider: a travelling wave up to the thumb, a
//! straight line after it. Seeking is offered only with a seek length;
//! without one the slider only displays.

use std::f32::consts::TAU;
use std::time::Duration;

use iced::widget::canvas::{self, Frame, Geometry, LineCap, Path, Stroke};
use iced::{mouse, Element, Length, Point, Rectangle, Renderer, Theme};
use obayebar::style;

use crate::media::Action;
use crate::Message;

/// Height of the slider canvas, and room for the wave's crests.
const SLIDER_HEIGHT: f32 = 28.0;
/// Horizontal distance between two crests of the elapsed wave.
const WAVELENGTH: f32 = 24.0;
/// Horizontal resolution of the wave polyline.
const WAVE_STEP: f32 = 2.0;
const TRACK_WIDTH: f32 = 3.0;
const THUMB_RADIUS: f32 = 6.0;

/// How far through the track `position` is, in `0.0..=1.0`.
pub fn progress(position: Duration, length: Option<Duration>) -> f32 {
    length
        .filter(|length| !length.is_zero())
        .map_or(0.0, |length| position.div_duration_f32(length).min(1.0))
}

/// Where along a slider of `width` the pointer at `x` points, in `0.0..=1.0`.
fn fraction_at(x: f32, width: f32) -> f32 {
    if width > 0.0 {
        (x / width).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The slider's usable span in its own local coordinates: the thumb's centre
/// travels from `.0` to `.1`, inset by its own radius on each side. Shared by
/// [`WaveSlider::draw`], which already works in local coordinates, and
/// [`WaveSlider::fraction`], which offsets a screen-space cursor position by
/// `bounds`'s origin first.
fn span(bounds: Rectangle) -> (f32, f32) {
    (THUMB_RADIUS, bounds.width - THUMB_RADIUS)
}

/// The elapsed part of the slider: a sine of `amplitude` around `mid_y`
/// sampled from `start` to `end`, both ends included.
fn wave_points(start: f32, end: f32, mid_y: f32, amplitude: f32, phase: f32) -> Vec<Point> {
    let y = |x: f32| amplitude.mul_add((x / WAVELENGTH).mul_add(TAU, -phase).sin(), mid_y);
    std::iter::successors(Some(start), |x| {
        Some(x + WAVE_STEP).filter(|next| *next < end)
    })
    .chain((end > start).then_some(end))
    .map(|x| Point::new(x, y(x)))
    .collect()
}

#[derive(Debug, Default)]
pub struct SliderState {
    dragging: Option<f32>,
}

struct WaveSlider {
    progress: f32,
    amplitude: f32,
    phase: f32,
    seek_length: Option<Duration>,
}

impl WaveSlider {
    fn fraction(bounds: Rectangle, x: f32) -> f32 {
        let (start, end) = span(bounds);
        fraction_at(x - bounds.x - start, end - start)
    }
}

impl canvas::Program<Message> for WaveSlider {
    type State = SliderState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let length = self.seek_length?;
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let position = cursor.position_over(bounds)?;
                state.dragging = Some(Self::fraction(bounds, position.x));
                Some(canvas::Action::request_redraw().and_capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                state.dragging.as_mut().map(|dragging| {
                    *dragging = Self::fraction(bounds, position.x);
                    canvas::Action::request_redraw()
                })
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let fraction = state.dragging.take()?;
                Some(
                    canvas::Action::publish(Message::Media(crate::media::Message::Control(
                        Action::Seek(length.mul_f32(fraction)),
                    )))
                    .and_capture(),
                )
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry<Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let mid = bounds.height / 2.0;
        let (start, end) = span(bounds);
        let thumb_x = (end - start).mul_add(state.dragging.unwrap_or(self.progress), start);

        let wave = Path::new(|builder| {
            let mut points =
                wave_points(start, thumb_x, mid, self.amplitude, self.phase).into_iter();
            if let Some(first) = points.next() {
                builder.move_to(first);
                points.for_each(|point| builder.line_to(point));
            }
        });
        let stroke = |color| {
            Stroke::default()
                .with_width(TRACK_WIDTH)
                .with_color(color)
                .with_line_cap(LineCap::Round)
        };
        frame.stroke(&wave, stroke(style::M3_PRIMARY));
        if thumb_x < end {
            frame.stroke(
                &Path::line(Point::new(thumb_x, mid), Point::new(end, mid)),
                stroke(style::with_alpha(style::M3_ON_SURFACE, 0.3)),
            );
        }
        frame.fill(
            &Path::circle(Point::new(thumb_x, mid), THUMB_RADIUS),
            style::M3_PRIMARY,
        );
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.dragging.is_some() {
            mouse::Interaction::Grabbing
        } else if self.seek_length.is_some() && cursor.is_over(bounds) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

/// Build the slider element at its fixed height.
pub fn view<'a>(
    progress: f32,
    amplitude: f32,
    phase: f32,
    seek_length: Option<Duration>,
) -> Element<'a, Message> {
    canvas::Canvas::new(WaveSlider {
        progress,
        amplitude,
        phase,
        seek_length,
    })
    .width(Length::Fill)
    .height(Length::Fixed(SLIDER_HEIGHT))
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_a_clamped_fraction() {
        let length = Some(Duration::from_secs(100));
        assert!((progress(Duration::from_secs(25), length) - 0.25).abs() < f32::EPSILON);
        assert!((progress(Duration::from_secs(250), length) - 1.0).abs() < f32::EPSILON);
        assert!(progress(Duration::from_secs(25), None).abs() < f32::EPSILON);
        assert!(progress(Duration::from_secs(25), Some(Duration::ZERO)).abs() < f32::EPSILON);
    }

    #[test]
    fn pointer_fraction_is_clamped_to_the_slider() {
        assert!((fraction_at(50.0, 200.0) - 0.25).abs() < f32::EPSILON);
        assert!(fraction_at(-10.0, 200.0).abs() < f32::EPSILON);
        assert!((fraction_at(900.0, 200.0) - 1.0).abs() < f32::EPSILON);
        assert!(fraction_at(10.0, 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_wave_spans_its_range_within_its_amplitude() {
        let points = wave_points(10.0, 100.0, 20.0, 4.0, 1.0);
        assert!(points.len() > 2);
        assert_eq!(points.first().map(|p| p.x), Some(10.0));
        assert_eq!(points.last().map(|p| p.x), Some(100.0));
        assert!(points
            .iter()
            .all(|p| (p.y - 20.0).abs() <= 4.0 + f32::EPSILON));
        assert!(points.iter().any(|p| (p.y - 20.0).abs() > 1.0));
        assert!(points.windows(2).all(|w| matches!(w, [a, b] if a.x < b.x)));
    }

    #[test]
    fn a_zero_amplitude_wave_is_a_straight_line() {
        let points = wave_points(0.0, 50.0, 8.0, 0.0, 2.0);
        assert!(points.iter().all(|p| (p.y - 8.0).abs() < f32::EPSILON));
    }

    #[test]
    fn an_empty_range_is_a_single_point() {
        assert_eq!(wave_points(30.0, 30.0, 8.0, 0.0, 0.0).len(), 1);
        assert_eq!(wave_points(30.0, 10.0, 8.0, 0.0, 0.0).len(), 1);
    }

    #[test]
    fn the_span_is_inset_by_the_thumb_radius_and_ignores_the_screen_offset() {
        let bounds = Rectangle::new(Point::new(10.0, 0.0), iced::Size::new(100.0, SLIDER_HEIGHT));
        assert_eq!(span(bounds), (THUMB_RADIUS, 100.0 - THUMB_RADIUS));
    }
}
