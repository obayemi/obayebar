//! Shared iced widget builders used across panels.

use crate::panel::PanelKind;
use crate::Message;
use iced::widget::canvas::{self, path::Arc, Frame, Geometry, LineCap, Path, Stroke};
use iced::widget::{button, container, mouse_area, text, toggler, Space};
use iced::{
    Alignment, Color, Element, Length, Padding, Point, Radians, Rectangle, Renderer, Theme,
};
use obayebar::style;

/// Start angle of the 3/4-circle gauge arc (bottom-left, at 135 degrees).
pub const GAUGE_ARC_START: f32 = std::f32::consts::PI * 0.75;
/// Total sweep of the 3/4-circle gauge arc (270 degrees, open at the bottom).
pub const GAUGE_ARC_SPAN: f32 = std::f32::consts::PI * 1.5;

/// Path for a sub-arc inside the standard 3/4-circle gauge layout.
pub fn gauge_arc(center: Point, radius: f32, start_offset: f32, sweep: f32) -> Path {
    Path::new(|builder| {
        let start_angle = GAUGE_ARC_START + start_offset;
        builder.arc(Arc {
            center,
            radius,
            start_angle: Radians(start_angle),
            end_angle: Radians(start_angle + sweep),
        });
    })
}

/// Canvas program for the 3/4-circle percentage gauge shared by the battery
/// and sysinfo panels: a faint background track plus a colored foreground arc
/// proportional to `percent` (0–100). Stroke width is fixed by `arc_width` so
/// callers can match it to the radius they pick.
#[derive(Debug)]
pub struct GaugeProgram {
    pub percent: f32,
    pub color: Color,
    pub arc_width: f32,
}

impl canvas::Program<Message> for GaugeProgram {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<Geometry<Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        let center = Point::new(bounds.width / 2.0, bounds.height / 2.0);
        let radius = (bounds.width.min(bounds.height) / 2.0) - self.arc_width;

        frame.stroke(
            &gauge_arc(center, radius, 0.0, GAUGE_ARC_SPAN),
            Stroke::default()
                .with_width(self.arc_width)
                .with_color(style::with_alpha(style::M3_ON_SURFACE, 0.12)),
        );

        let fill_angle = GAUGE_ARC_SPAN * (self.percent / 100.0);
        if fill_angle > 0.01 {
            frame.stroke(
                &gauge_arc(center, radius, 0.0, fill_angle),
                Stroke::default()
                    .with_width(self.arc_width)
                    .with_color(self.color)
                    .with_line_cap(LineCap::Round),
            );
        }

        vec![frame.into_geometry()]
    }
}

/// Standard panel header: an icon and a title, ready for trailing content.
///
/// Returns the `Row` rather than a finished element because the six panels
/// differ in what follows the title — nothing, a toggler, or a count badge —
/// and hiding that behind a builder would need a parameter per variant. The
/// icon colour is a parameter because the GitLab panel uses the tertiary
/// accent while the others use primary.
pub fn panel_header<'a>(
    icon: &'a str,
    title: &'a str,
    icon_color: Color,
) -> iced::widget::Row<'a, Message> {
    iced::widget::row![
        text(icon)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(icon_color),
        text(title)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_ON_SURFACE),
    ]
    .spacing(style::SPACING_SMALLER)
    .align_y(Alignment::Center)
}

/// 1px horizontal line used between panel sections.
pub fn separator<'a>() -> Element<'a, Message> {
    container(Space::new().width(Length::Fill).height(1.0))
        .style(|_theme| container::Style {
            background: Some(iced::Background::Color(style::with_alpha(
                style::M3_OUTLINE_VARIANT,
                0.5,
            ))),
            ..container::Style::default()
        })
        .into()
}

/// Wrap a rendered panel with the standard popup scaffolding:
///   - outer `panel_wrapper_container` style so the compositor includes the
///     gap area in the input region,
///   - `PANEL_GAP` padding on the side adjacent to the bar,
///   - `mouse_area` reporting pointer enter/leave for this surface.
///
/// Both edges are reported, not just the leave. iced publishes `on_exit`
/// strictly on a `was_hovered → !is_hovered` transition, so a panel the pointer
/// never entered can never fire one — which is why this used to be the *only*
/// dismissal producer and a panel the pointer walked past stayed open forever.
/// Dismissal is now decided in `update()` from the pointer's location across
/// both the bar trigger and the panel.
pub fn panel_with_exit(kind: PanelKind, content: Element<'_, Message>) -> Element<'_, Message> {
    mouse_area(
        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(Alignment::End)
            .padding(Padding {
                top: 0.0,
                right: 0.0,
                bottom: style::PANEL_GAP,
                left: style::PANEL_GAP,
            })
            .style(style::panel_wrapper_container),
    )
    .on_enter(Message::PanelPointerEntered(kind))
    .on_exit(Message::PanelPointerLeftPanel(kind))
    .into()
}

/// Panel flavour of `style::hover_button`, pinned to the panels' corner
/// radius so the six call sites do not each repeat it.
pub fn hover_button_style(
    bg: Color,
    text_color: Color,
) -> impl Fn(&iced::Theme, button::Status) -> button::Style + 'static {
    style::hover_button(bg, text_color, style::ROUNDING_SMALL)
}

/// Small icon-only button with `hover_button_style` and transparent baseline.
pub fn icon_button(icon: &str, color: Color, message: Message) -> Element<'_, Message> {
    button(
        text(icon)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_NORMAL)
            .color(color)
            .align_x(Alignment::Center),
    )
    .on_press(message)
    .style(hover_button_style(Color::TRANSPARENT, color))
    .padding(style::PADDING_SMALL)
    .into()
}

/// Material-3 styled toggler with on/off color scheme.
pub fn styled_toggler<F>(enabled: bool, on_toggle: F) -> Element<'static, Message>
where
    F: Fn(bool) -> Message + 'static,
{
    toggler(enabled)
        .on_toggle(on_toggle)
        .size(style::FONT_SIZE_LARGE)
        .style(|_theme, status| {
            let is_on = matches!(
                status,
                iced::widget::toggler::Status::Active { is_toggled: true }
                    | iced::widget::toggler::Status::Hovered { is_toggled: true }
            );
            if is_on {
                iced::widget::toggler::Style {
                    background: iced::Background::Color(style::M3_PRIMARY),
                    foreground: iced::Background::Color(style::M3_ON_PRIMARY),
                    background_border_width: 0.0,
                    background_border_color: Color::TRANSPARENT,
                    foreground_border_width: 0.0,
                    foreground_border_color: Color::TRANSPARENT,
                    text_color: None,
                    border_radius: None,
                    padding_ratio: 0.15,
                }
            } else {
                iced::widget::toggler::Style {
                    background: iced::Background::Color(style::M3_SURFACE_CONTAINER_HIGHEST),
                    foreground: iced::Background::Color(style::M3_OUTLINE),
                    background_border_width: 2.0,
                    background_border_color: style::M3_OUTLINE,
                    foreground_border_width: 0.0,
                    foreground_border_color: Color::TRANSPARENT,
                    text_color: None,
                    border_radius: None,
                    padding_ratio: 0.15,
                }
            }
        })
        .into()
}
