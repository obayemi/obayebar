use crate::services::bluetooth::BluetoothInfo;
use crate::style;
use crate::Message;
use iced::widget::{button, column, container, mouse_area, row, text, Space};
use iced::{Alignment, Border, Element, Length, Padding};

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

fn device_icon(icon_hint: &str) -> &'static str {
    match icon_hint {
        s if s.contains("headset") || s.contains("headphone") => style::ICON_VOLUME_UP,
        s if s.contains("keyboard") => style::ICON_SETTINGS,
        s if s.contains("phone") => style::ICON_LANGUAGE,
        _ => style::ICON_BLUETOOTH,
    }
}

fn device_entry<'a>(
    alias: &'a str,
    icon_hint: &'a str,
    connected: bool,
    battery: Option<u8>,
    path: &'a str,
) -> Element<'a, Message> {
    let (bg, text_color, icon_color) = if connected {
        (
            style::with_alpha(style::M3_PRIMARY, 0.15),
            style::M3_PRIMARY,
            style::M3_PRIMARY,
        )
    } else {
        (
            iced::Color::TRANSPARENT,
            style::M3_ON_SURFACE,
            style::M3_ON_SURFACE_VARIANT,
        )
    };

    let icon = text(device_icon(icon_hint))
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_NORMAL)
        .color(icon_color);

    let mut info =
        column![text(alias).size(style::FONT_SIZE_NORMAL).color(text_color)].width(Length::Fill);

    if let Some(bat) = battery {
        info = info.push(
            text(format!("{bat}%"))
                .size(style::FONT_SIZE_SMALL)
                .color(style::M3_ON_SURFACE_VARIANT),
        );
    }

    let content = row![icon, info]
        .spacing(style::SPACING_SMALLER)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    let path_owned = path.to_string();
    button(content)
        .on_press(Message::BluetoothToggleDevice {
            path: path_owned,
            connected,
        })
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
        .padding([style::PADDING_SMALL, style::PADDING_NORMAL])
        .width(Length::Fill)
        .into()
}

pub fn view(bt: &BluetoothInfo) -> Element<'_, Message> {
    let header = row![
        text(bt.icon_name)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_PRIMARY),
        text("Bluetooth")
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_ON_SURFACE),
    ]
    .spacing(style::SPACING_SMALLER)
    .align_y(Alignment::Center);

    let mut content = column![header, separator()]
        .spacing(style::SPACING_NORMAL)
        .width(Length::Fill);

    if bt.devices.is_empty() {
        content = content.push(
            text(if bt.powered {
                "No paired devices"
            } else {
                "Bluetooth is off"
            })
            .size(style::FONT_SIZE_NORMAL)
            .color(style::M3_ON_SURFACE_VARIANT),
        );
    } else {
        let mut device_list = column![text("Devices")
            .size(style::FONT_SIZE_SMALLER)
            .color(style::M3_ON_SURFACE_VARIANT)]
        .spacing(2.0)
        .width(Length::Fill);

        for dev in &bt.devices {
            device_list = device_list.push(device_entry(
                &dev.alias,
                &dev.icon,
                dev.connected,
                dev.battery,
                &dev.path,
            ));
        }

        content = content.push(device_list);
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
            }),
    )
    .on_exit(Message::CloseAllPanels)
    .into()
}
