use crate::services::sysinfo::{self, SysInfo};
use crate::style;
use crate::Message;
use iced::widget::canvas::{self, Frame, Geometry, LineCap, Path, Stroke};
use iced::widget::{column, container, mouse_area, row, text, Stack};
use iced::{Alignment, Element, Length, Padding, Point, Rectangle, Renderer, Theme};

const GAUGE_SIZE: f32 = 90.0;
const ARC_WIDTH: f32 = 7.0;
/// The arc spans 270 degrees (3/4 of a circle), open at the bottom
const ARC_SPAN: f32 = std::f32::consts::PI * 1.5;
/// Start angle: 135 degrees (bottom-left)
const ARC_START: f32 = std::f32::consts::PI * 0.75;

struct UsageGaugeProgram {
    percent: f32,
    color: iced::Color,
}

impl canvas::Program<Message> for UsageGaugeProgram {
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

        // Foreground arc
        let fill_angle = ARC_SPAN * (self.percent / 100.0);
        if fill_angle > 0.01 {
            frame.stroke(
                &arc_path(center, radius, 0.0, fill_angle),
                Stroke::default()
                    .with_width(ARC_WIDTH)
                    .with_color(self.color)
                    .with_line_cap(LineCap::Round),
            );
        }

        vec![frame.into_geometry()]
    }
}

fn arc_path(center: Point, radius: f32, start_offset: f32, sweep: f32) -> Path {
    Path::new(|builder| {
        let start = ARC_START + start_offset;
        let steps = 64;
        #[allow(clippy::cast_precision_loss)]
        let step_angle = sweep / steps as f32;
        let first = Point::new(
            radius.mul_add(start.cos(), center.x),
            radius.mul_add(start.sin(), center.y),
        );
        builder.move_to(first);
        for i in 1..=steps {
            #[allow(clippy::cast_precision_loss)]
            let angle = step_angle.mul_add(i as f32, start);
            builder.line_to(Point::new(
                radius.mul_add(angle.cos(), center.x),
                radius.mul_add(angle.sin(), center.y),
            ));
        }
    })
}

fn usage_color(percent: f32) -> iced::Color {
    if percent >= 90.0 {
        style::M3_ERROR
    } else if percent >= 70.0 {
        style::M3_TERTIARY
    } else {
        style::M3_PRIMARY
    }
}

fn temp_color(temp_c: f32) -> iced::Color {
    if temp_c >= 90.0 {
        style::M3_ERROR
    } else if temp_c >= 70.0 {
        style::M3_TERTIARY
    } else {
        style::M3_ON_SURFACE_VARIANT
    }
}

fn gauge_widget<'a>(
    percent: f32,
    icon: &'static str,
    label: &'static str,
    temp_c: Option<f32>,
) -> Element<'a, Message> {
    let color = usage_color(percent);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let pct = percent.round() as u32;
    let pct_text = text(format!("{pct}%"))
        .size(style::FONT_SIZE_LARGER)
        .color(style::M3_ON_SURFACE)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center);

    let icon_text = text(icon)
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_SMALL)
        .color(color)
        .align_x(Alignment::Center);

    let overlay = column![pct_text, icon_text]
        .align_x(Alignment::Center)
        .spacing(1.0);

    let gauge_canvas = canvas::Canvas::new(UsageGaugeProgram { percent, color })
        .width(Length::Fixed(GAUGE_SIZE))
        .height(Length::Fixed(GAUGE_SIZE));

    let gauge = container(Stack::with_children(vec![
        gauge_canvas.into(),
        container(overlay)
            .width(Length::Fixed(GAUGE_SIZE))
            .height(Length::Fixed(GAUGE_SIZE))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .into(),
    ]));

    let label_text = text(label)
        .size(style::FONT_SIZE_SMALL)
        .color(style::M3_ON_SURFACE_VARIANT)
        .align_x(Alignment::Center);

    let mut col = column![gauge, label_text]
        .spacing(2.0)
        .align_x(Alignment::Center)
        .width(Length::Fill);

    if let Some(t) = temp_c {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let temp_val = t.round() as u32;
        let tc = temp_color(t);
        let temp_row = row![
            text(style::ICON_THERMOSTAT)
                .font(style::ICON_FONT)
                .size(style::FONT_SIZE_SMALL)
                .color(tc),
            text(format!("{temp_val}\u{00B0}C"))
                .size(style::FONT_SIZE_SMALL)
                .color(tc),
        ]
        .spacing(1.0)
        .align_y(Alignment::Center);
        col = col.push(
            container(temp_row)
                .width(Length::Fill)
                .align_x(Alignment::Center),
        );
    }

    col.into()
}

fn net_widget(sysinfo: &SysInfo) -> Element<'_, Message> {
    let rx_rate = sysinfo::format_rate(sysinfo.net_rx_rate);
    let tx_rate = sysinfo::format_rate(sysinfo.net_tx_rate);

    let rx_row = row![
        text(style::ICON_ARROW_DOWNWARD)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_SMALL)
            .color(style::M3_PRIMARY),
        text(rx_rate)
            .size(style::FONT_SIZE_SMALL)
            .color(style::M3_ON_SURFACE),
    ]
    .spacing(2.0)
    .align_y(Alignment::Center);

    let tx_row = row![
        text(style::ICON_ARROW_UPWARD)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_SMALL)
            .color(style::M3_TERTIARY),
        text(tx_rate)
            .size(style::FONT_SIZE_SMALL)
            .color(style::M3_ON_SURFACE),
    ]
    .spacing(2.0)
    .align_y(Alignment::Center);

    let label_text = text("Network")
        .size(style::FONT_SIZE_SMALL)
        .color(style::M3_ON_SURFACE_VARIANT)
        .align_x(Alignment::Center);

    let net_content = column![rx_row, tx_row]
        .spacing(style::SPACING_SMALL)
        .align_x(Alignment::Center);

    // Sized container matching gauge cells
    let net_box = container(net_content)
        .width(Length::Fixed(GAUGE_SIZE))
        .height(Length::Fixed(GAUGE_SIZE))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center);

    column![net_box, label_text]
        .spacing(2.0)
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .into()
}

pub fn view(sysinfo: &SysInfo) -> Element<'_, Message> {
    let header = row![
        text(style::ICON_SPEED)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_PRIMARY),
        text("System")
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_ON_SURFACE),
    ]
    .spacing(style::SPACING_SMALLER)
    .align_y(Alignment::Center);

    let top_row = row![
        gauge_widget(
            sysinfo.cpu_percent,
            style::ICON_SPEED,
            "CPU",
            sysinfo.cpu_temp_c,
        ),
        gauge_widget(
            sysinfo.gpu_percent,
            style::ICON_GPU,
            "GPU",
            sysinfo.gpu_temp_c,
        ),
    ]
    .spacing(style::SPACING_SMALL);

    let bottom_row = row![
        gauge_widget(sysinfo.ram_percent, style::ICON_MEMORY, "RAM", None),
        net_widget(sysinfo),
    ]
    .spacing(style::SPACING_SMALL);

    let content = column![header, top_row, bottom_row]
        .spacing(style::SPACING_NORMAL)
        .width(Length::Fill)
        .align_x(Alignment::Center);

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
