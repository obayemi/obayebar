use crate::services::battery::BatteryInfo;
use crate::style;
use crate::Message;
use iced::widget::canvas::{self, path::Arc, Frame, Geometry, Path, Stroke};
use iced::widget::{button, column, container, mouse_area, row, text, Space, Stack};
use iced::{
    Alignment, Border, Element, Length, Padding, Point, Radians, Rectangle, Renderer, Theme,
};

const GAUGE_SIZE: f32 = 140.0;
const ARC_WIDTH: f32 = 10.0;
/// The arc spans 270 degrees (3/4 of a circle), open at the bottom
const ARC_SPAN: f32 = std::f32::consts::PI * 1.5;
/// Start angle: 135 degrees (bottom-left)
const ARC_START: f32 = std::f32::consts::PI * 0.75;

struct GaugeProgram {
    percentage: f64,
    charging: bool,
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
        let radius = (bounds.width.min(bounds.height) / 2.0) - ARC_WIDTH;

        // Background track
        frame.stroke(
            &arc_path(center, radius, 0.0, ARC_SPAN),
            Stroke::default()
                .with_width(ARC_WIDTH)
                .with_color(style::with_alpha(style::M3_ON_SURFACE, 0.12)),
        );

        // Foreground arc (percentage)
        #[allow(clippy::cast_possible_truncation)]
        let fill_angle = ARC_SPAN * (self.percentage as f32 / 100.0);
        let color = if self.charging {
            style::M3_PRIMARY
        } else if self.percentage <= 20.0 {
            style::M3_ERROR
        } else {
            style::M3_PRIMARY
        };

        if fill_angle > 0.01 {
            frame.stroke(
                &arc_path(center, radius, 0.0, fill_angle),
                Stroke::default()
                    .with_width(ARC_WIDTH)
                    .with_color(color)
                    .with_line_cap(iced::widget::canvas::LineCap::Round),
            );
        }

        vec![frame.into_geometry()]
    }
}

fn arc_path(center: Point, radius: f32, start_offset: f32, sweep: f32) -> Path {
    Path::new(|builder| {
        let start_angle = ARC_START + start_offset;
        builder.arc(Arc {
            center,
            radius,
            start_angle: Radians(start_angle),
            end_angle: Radians(start_angle + sweep),
        });
    })
}

fn format_duration(seconds: i64) -> String {
    if seconds <= 0 {
        return String::new();
    }
    let hours = seconds / 3600;
    let mins = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {mins:02}m")
    } else {
        format!("{mins}m")
    }
}

fn separator<'a>() -> Element<'a, Message> {
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

fn profile_label(name: &str) -> &str {
    match name {
        "power-saver" => "Saver",
        "balanced" => "Balanced",
        "performance" => "Perf",
        other => other,
    }
}

fn profile_icon(name: &str) -> &'static str {
    match name {
        "power-saver" => style::ICON_ECO,
        "performance" => style::ICON_SPEED,
        _ => style::ICON_BOLT,
    }
}

fn profile_button(name: &str, is_active: bool) -> Element<'_, Message> {
    let (bg, text_color) = if is_active {
        (
            style::with_alpha(style::M3_PRIMARY, 0.15),
            style::M3_PRIMARY,
        )
    } else {
        (iced::Color::TRANSPARENT, style::M3_ON_SURFACE)
    };

    let label = profile_label(name);
    let icon = profile_icon(name);
    let profile_name = name.to_string();

    let content = column![
        text(icon)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_NORMAL)
            .color(text_color)
            .align_x(Alignment::Center),
        text(label)
            .size(style::FONT_SIZE_SMALL)
            .color(text_color)
            .align_x(Alignment::Center),
    ]
    .align_x(Alignment::Center)
    .spacing(2.0)
    .width(Length::Fill);

    button(content)
        .on_press(Message::SetPowerProfile(profile_name))
        .style(move |_theme, status| {
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
        })
        .padding([style::PADDING_SMALL, style::PADDING_SMALLER])
        .width(Length::Fill)
        .into()
}

fn gauge_widget(battery: &BatteryInfo) -> Element<'_, Message> {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let pct = battery.percentage.round() as u32;
    let pct_text = text(format!("{pct}%"))
        .size(28.0)
        .color(style::M3_ON_SURFACE)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center);

    let status_label = if battery.charging {
        text(style::ICON_BOLT)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_SMALLER)
            .color(style::M3_PRIMARY)
            .align_x(Alignment::Center)
    } else {
        text(style::ICON_BATTERY_FULL)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_SMALLER)
            .color(style::M3_ON_SURFACE_VARIANT)
            .align_x(Alignment::Center)
    };

    let pct_overlay = column![pct_text, status_label]
        .align_x(Alignment::Center)
        .spacing(2.0);

    let gauge_canvas = canvas::Canvas::new(GaugeProgram {
        percentage: battery.percentage,
        charging: battery.charging,
    })
    .width(Length::Fixed(GAUGE_SIZE))
    .height(Length::Fixed(GAUGE_SIZE));

    container(Stack::with_children(vec![
        gauge_canvas.into(),
        container(pct_overlay)
            .width(Length::Fixed(GAUGE_SIZE))
            .height(Length::Fixed(GAUGE_SIZE))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .into(),
    ]))
    .width(Length::Fill)
    .align_x(Alignment::Center)
    .into()
}

pub fn view(battery: &BatteryInfo) -> Element<'_, Message> {
    let header = iced::widget::row![
        text(battery.icon_name)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_PRIMARY),
        text("Battery")
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_ON_SURFACE),
    ]
    .spacing(style::SPACING_SMALLER)
    .align_y(Alignment::Center);

    // Time remaining text
    let time_text = if battery.charging && battery.time_to_full > 0 {
        let dur = format_duration(battery.time_to_full);
        format!("{dur} until full")
    } else if !battery.charging && battery.time_to_empty > 0 {
        let dur = format_duration(battery.time_to_empty);
        format!("{dur} remaining")
    } else if battery.charging {
        "Charging".to_string()
    } else {
        "On battery".to_string()
    };

    let time_label = text(time_text)
        .size(style::FONT_SIZE_SMALLER)
        .color(style::M3_ON_SURFACE_VARIANT)
        .align_x(Alignment::Center)
        .width(Length::Fill);

    let mut content = column![header, gauge_widget(battery), time_label]
        .spacing(style::SPACING_NORMAL)
        .width(Length::Fill)
        .align_x(Alignment::Center);

    // Power profile selector
    if let Some(ref profiles) = battery.power_profiles {
        content = content.push(separator());
        content = content.push(
            text("Power profile")
                .size(style::FONT_SIZE_SMALLER)
                .color(style::M3_ON_SURFACE_VARIANT),
        );
        let mut profile_row = row![].spacing(4.0).width(Length::Fill);
        for profile in &profiles.available_profiles {
            let is_active = profile == &profiles.active_profile;
            profile_row = profile_row.push(profile_button(profile, is_active));
        }
        content = content.push(profile_row);
    }

    let panel = container(content)
        .padding(style::PADDING_LARGE)
        .width(Length::Fill)
        .height(Length::Shrink)
        .style(style::audio_panel_container);

    mouse_area(
        container(panel)
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
