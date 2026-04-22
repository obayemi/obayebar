//! Shared iced widget builders used across panels.

use crate::Message;
use iced::widget::canvas::{path::Arc, Path};
use iced::widget::{button, container, mouse_area, text, toggler, Space};
use iced::{Alignment, Border, Color, Element, Length, Padding, Point, Radians};
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
///   - `mouse_area` whose `on_exit` dismisses all panels.
pub fn panel_with_exit(content: Element<'_, Message>) -> Element<'_, Message> {
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
    .on_exit(Message::CloseAllPanels)
    .into()
}

/// Reusable button style closure: transparent (or caller-supplied) background
/// that switches to an 8%-alpha `M3_ON_SURFACE` wash on hover/press.
pub fn hover_button_style(
    bg: Color,
    text_color: Color,
) -> impl Fn(&iced::Theme, button::Status) -> button::Style + 'static {
    move |_theme, status| {
        let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let bg_color = if hover {
            style::with_alpha(style::M3_ON_SURFACE, 0.08)
        } else {
            bg
        };
        button::Style {
            background: Some(iced::Background::Color(bg_color)),
            text_color,
            border: Border {
                radius: style::ROUNDING_SMALL.into(),
                ..Border::default()
            },
            shadow: iced::Shadow::default(),
            snap: false,
        }
    }
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
