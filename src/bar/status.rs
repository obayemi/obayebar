use crate::services::audio::AudioInfo;
use crate::services::battery::BatteryInfo;
use crate::services::bluetooth::BluetoothInfo;
use crate::services::network::NetworkInfo;
use crate::services::sysinfo::SysInfo;
use crate::Message;
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke};
use iced::widget::{column, container, mouse_area, text};
use iced::{mouse, Alignment, Color, Element, Length, Pixels, Point, Rectangle, Renderer, Theme};
use obayebar::style;

/// Threshold above which usage is considered elevated.
const ELEVATED_THRESHOLD: f32 = 70.0;
/// Threshold above which usage is considered critical.
const CRITICAL_THRESHOLD: f32 = 90.0;
/// Volume change per scroll line on the bar audio icon.
const VOLUME_SCROLL_STEP: f32 = 0.05;

/// Canvas size for the split icon (matches single icon visual size).
const SPLIT_SIZE: f32 = style::FONT_SIZE_LARGE + 6.0;

fn usage_color(percent: f32) -> Color {
    if percent >= CRITICAL_THRESHOLD {
        style::M3_ERROR
    } else if percent >= ELEVATED_THRESHOLD {
        style::M3_TERTIARY
    } else {
        style::M3_SECONDARY
    }
}

/// Render a single icon at the standard bar size.
fn single_icon(icon: &str, color: Color) -> Element<'_, Message> {
    text(icon)
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_LARGE)
        .color(color)
        .align_x(Alignment::Center)
        .into()
}

/// Canvas program that draws two icon glyphs split diagonally:
/// CPU (top-right triangle) and RAM (bottom-left triangle).
struct DiagonalSplitProgram {
    cpu_icon: &'static str,
    cpu_color: Color,
    ram_icon: &'static str,
    ram_color: Color,
}

impl canvas::Program<Message> for DiagonalSplitProgram {
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
        let s = bounds.width.min(bounds.height);
        let center = Point::new(s / 2.0, s / 2.0);

        // Offset each icon away from the diagonal so only the correct half
        // peeks into the visible triangle. The diagonal goes top-right → bottom-left.
        let offset = s * 0.18;

        // RAM icon: shifted toward bottom-left
        frame.fill_text(canvas::Text {
            content: self.ram_icon.to_string(),
            position: Point::new(center.x - offset, center.y + offset),
            color: self.ram_color,
            size: Pixels(style::FONT_SIZE_LARGE),
            font: style::ICON_FONT,
            align_x: iced::alignment::Horizontal::Center.into(),
            align_y: iced::alignment::Vertical::Center,
            ..canvas::Text::default()
        });

        // Cover the top-right half of the RAM icon with a filled triangle
        // so only its bottom-left portion remains visible.
        frame.fill(
            &top_right_triangle(s),
            style::with_alpha(style::M3_SURFACE_CONTAINER, 0.85),
        );

        // CPU icon: shifted toward top-right
        frame.fill_text(canvas::Text {
            content: self.cpu_icon.to_string(),
            position: Point::new(center.x + offset, center.y - offset),
            color: self.cpu_color,
            size: Pixels(style::FONT_SIZE_LARGE),
            font: style::ICON_FONT,
            align_x: iced::alignment::Horizontal::Center.into(),
            align_y: iced::alignment::Vertical::Center,
            ..canvas::Text::default()
        });

        // Cover the bottom-left half of the CPU icon with a filled triangle
        // so only its top-right portion remains visible.
        frame.fill(
            &bottom_left_triangle(s),
            style::with_alpha(style::M3_SURFACE_CONTAINER, 0.85),
        );

        // Draw a subtle diagonal separator line (top-right to bottom-left)
        frame.stroke(
            &Path::line(Point::new(s, 0.0), Point::new(0.0, s)),
            Stroke::default()
                .with_width(1.0)
                .with_color(style::with_alpha(style::M3_OUTLINE_VARIANT, 0.6)),
        );

        vec![frame.into_geometry()]
    }
}

/// Triangle covering the top-right half (above the diagonal from top-right to bottom-left).
fn top_right_triangle(size: f32) -> Path {
    Path::new(|b| {
        b.move_to(Point::new(0.0, 0.0));
        b.line_to(Point::new(size, 0.0));
        b.line_to(Point::new(size, size));
        b.close();
    })
}

/// Triangle covering the bottom-left half (below the diagonal from top-right to bottom-left).
fn bottom_left_triangle(size: f32) -> Path {
    Path::new(|b| {
        b.move_to(Point::new(0.0, 0.0));
        b.line_to(Point::new(size, size));
        b.line_to(Point::new(0.0, size));
        b.close();
    })
}

/// Find the worst elevated metric to display in the bar icon.
/// When both CPU and RAM are elevated, shows a diagonal split icon.
fn sysinfo_icon_view(sysinfo: &SysInfo) -> Element<'static, Message> {
    let cpu_high = sysinfo.cpu_percent >= ELEVATED_THRESHOLD;
    let gpu_high = sysinfo.gpu_percent >= ELEVATED_THRESHOLD;
    let ram_high = sysinfo.ram_percent >= ELEVATED_THRESHOLD;

    // CPU + RAM both elevated → diagonal split
    if cpu_high && ram_high {
        return canvas::Canvas::new(DiagonalSplitProgram {
            cpu_icon: style::ICON_SPEED,
            cpu_color: usage_color(sysinfo.cpu_percent),
            ram_icon: style::ICON_MEMORY,
            ram_color: usage_color(sysinfo.ram_percent),
        })
        .width(Length::Fixed(SPLIT_SIZE))
        .height(Length::Fixed(SPLIT_SIZE))
        .into();
    }

    // Single worst metric
    if cpu_high {
        return single_icon(style::ICON_SPEED, usage_color(sysinfo.cpu_percent));
    }
    if ram_high {
        return single_icon(style::ICON_MEMORY, usage_color(sysinfo.ram_percent));
    }
    if gpu_high {
        return single_icon(style::ICON_GPU, usage_color(sysinfo.gpu_percent));
    }

    single_icon(style::ICON_CHECK_CIRCLE, style::M3_SECONDARY)
}

pub fn view(
    battery: &BatteryInfo,
    network: &NetworkInfo,
    audio: &AudioInfo,
    bluetooth: &BluetoothInfo,
    sysinfo: &SysInfo,
    monitor: Option<&str>,
) -> Element<'static, Message> {
    let mut icons = column![]
        .spacing(style::SPACING_SMALLER / 2.0)
        .align_x(Alignment::Center);

    let audio_volume = audio.volume;
    let audio_icon = mouse_area(
        text(audio.icon_name)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_SECONDARY)
            .align_x(Alignment::Center),
    )
    .on_enter(Message::AudioPanelOpen(monitor.map(String::from)))
    .on_press(Message::AudioOpenPavucontrol)
    .on_scroll(move |delta| {
        let dy = match delta {
            mouse::ScrollDelta::Lines { y, .. } => y,
            mouse::ScrollDelta::Pixels { y, .. } => y / 120.0,
        };
        let new_vol = (audio_volume + dy * VOLUME_SCROLL_STEP).clamp(0.0, 1.0);
        Message::AudioSetVolume(new_vol)
    });

    let network_icon = mouse_area(
        text(network.icon_name)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_SECONDARY)
            .align_x(Alignment::Center),
    )
    .on_enter(Message::NetworkPanelOpen(monitor.map(String::from)));

    let bluetooth_icon = mouse_area(
        text(bluetooth.icon_name)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_SECONDARY)
            .align_x(Alignment::Center),
    )
    .on_enter(Message::BluetoothPanelOpen(monitor.map(String::from)));

    let sysinfo_icon = mouse_area(sysinfo_icon_view(sysinfo))
        .on_enter(Message::SysinfoPanelOpen(monitor.map(String::from)));

    icons = icons.push(audio_icon);
    icons = icons.push(bluetooth_icon);
    icons = icons.push(network_icon);
    icons = icons.push(sysinfo_icon);

    if battery.present {
        let battery_color = if battery.percentage <= 20.0 {
            style::M3_ERROR
        } else {
            style::M3_SECONDARY
        };
        let battery_icon = mouse_area(
            text(battery.icon_name)
                .font(style::ICON_FONT)
                .size(style::FONT_SIZE_LARGE)
                .color(battery_color)
                .align_x(Alignment::Center),
        )
        .on_enter(Message::BatteryPanelOpen(monitor.map(String::from)));
        icons = icons.push(battery_icon);
    }

    container(icons)
        .padding(style::PADDING_NORMAL)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .style(style::pill_container)
        .into()
}
