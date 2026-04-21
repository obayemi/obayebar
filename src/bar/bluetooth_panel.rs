use crate::services::bluetooth::BluetoothInfo;
use crate::Message;
use iced::widget::{button, column, container, mouse_area, row, text, toggler, Space};
use iced::{Alignment, Border, Element, Length, Padding};
use obayebar::style;

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

fn icon_button(icon: &str, color: iced::Color, message: Message) -> Element<'_, Message> {
    button(
        text(icon)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_NORMAL)
            .color(color)
            .align_x(Alignment::Center),
    )
    .on_press(message)
    .style(move |_theme, status| {
        let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(iced::Background::Color(if hover {
                style::with_alpha(style::M3_ON_SURFACE, 0.08)
            } else {
                iced::Color::TRANSPARENT
            })),
            text_color: color,
            border: Border {
                radius: style::ROUNDING_SMALL.into(),
                ..Border::default()
            },
            shadow: iced::Shadow::default(),
            snap: false,
        }
    })
    .padding(style::PADDING_SMALL)
    .into()
}

fn device_entry(dev: &crate::services::bluetooth::BluetoothDevice) -> Element<'_, Message> {
    let (bg, text_color, icon_color) = if dev.connected {
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

    let icon = text(device_icon(&dev.icon))
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_NORMAL)
        .color(icon_color);

    let mut info = column![text(&*dev.alias)
        .size(style::FONT_SIZE_NORMAL)
        .color(text_color)]
    .width(Length::Fill);

    if let Some(bat) = dev.battery {
        info = info.push(
            text(format!("{bat}%"))
                .size(style::FONT_SIZE_SMALL)
                .color(style::M3_ON_SURFACE_VARIANT),
        );
    }

    // Action buttons
    let connect_icon = if dev.connected {
        style::ICON_BLUETOOTH_CONNECTED
    } else {
        style::ICON_BLUETOOTH
    };
    let connect_color = if dev.connected {
        style::M3_PRIMARY
    } else {
        style::M3_ON_SURFACE_VARIANT
    };

    let mut actions = row![].spacing(2.0).align_y(Alignment::Center);

    actions = actions.push(icon_button(
        connect_icon,
        connect_color,
        Message::BluetoothToggleDevice {
            path: dev.path.clone(),
            connected: dev.connected,
        },
    ));

    if dev.paired {
        actions = actions.push(icon_button(
            style::ICON_DELETE,
            style::M3_ON_SURFACE_VARIANT,
            Message::BluetoothForgetDevice(dev.path.clone()),
        ));
    }

    let content = row![icon, info, actions]
        .spacing(style::SPACING_SMALLER)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    container(content)
        .style(move |_theme| container::Style {
            background: Some(iced::Background::Color(bg)),
            border: Border {
                radius: style::ROUNDING_SMALL.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .padding([style::PADDING_SMALL, style::PADDING_NORMAL])
        .width(Length::Fill)
        .into()
}

/// Unpaired device entry with a connect/pair button.
fn nearby_entry(dev: &crate::services::bluetooth::BluetoothDevice) -> Element<'_, Message> {
    let icon = text(device_icon(&dev.icon))
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_NORMAL)
        .color(style::M3_ON_SURFACE_VARIANT);

    let info = column![text(&*dev.alias)
        .size(style::FONT_SIZE_NORMAL)
        .color(style::M3_ON_SURFACE)]
    .width(Length::Fill);

    let pair_btn = icon_button(
        style::ICON_BLUETOOTH,
        style::M3_ON_SURFACE_VARIANT,
        Message::BluetoothToggleDevice {
            path: dev.path.clone(),
            connected: false,
        },
    );

    let content = row![icon, info, pair_btn]
        .spacing(style::SPACING_SMALLER)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    container(content)
        .padding([style::PADDING_SMALL, style::PADDING_NORMAL])
        .width(Length::Fill)
        .into()
}

fn power_toggle(powered: bool) -> Element<'static, Message> {
    toggler(powered)
        .on_toggle(Message::BluetoothSetPowered)
        .size(style::FONT_SIZE_LARGE)
        .style(move |_theme, status| {
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
                    background_border_color: iced::Color::TRANSPARENT,
                    foreground_border_width: 0.0,
                    foreground_border_color: iced::Color::TRANSPARENT,
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
                    foreground_border_color: iced::Color::TRANSPARENT,
                    text_color: None,
                    border_radius: None,
                    padding_ratio: 0.15,
                }
            }
        })
        .into()
}

fn discovery_button(discovering: bool) -> Element<'static, Message> {
    let (label, icon, text_color, bg) = if discovering {
        (
            "Scanning...",
            style::ICON_BLUETOOTH_SEARCHING,
            style::M3_PRIMARY,
            style::with_alpha(style::M3_PRIMARY, 0.15),
        )
    } else {
        (
            "Scan for devices",
            style::ICON_BLUETOOTH_SEARCHING,
            style::M3_ON_SURFACE,
            iced::Color::TRANSPARENT,
        )
    };

    let content = row![
        text(icon)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_NORMAL)
            .color(text_color),
        text(label).size(style::FONT_SIZE_NORMAL).color(text_color),
    ]
    .spacing(style::SPACING_SMALLER)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    button(content)
        .on_press(Message::BluetoothSetDiscovery(!discovering))
        .style(move |_theme, status| {
            let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: Some(iced::Background::Color(if hover {
                    style::with_alpha(style::M3_ON_SURFACE, 0.08)
                } else {
                    bg
                })),
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
        Space::new().width(Length::Fill),
        power_toggle(bt.powered),
    ]
    .spacing(style::SPACING_SMALLER)
    .align_y(Alignment::Center);

    let mut content = column![header, separator()]
        .spacing(style::SPACING_NORMAL)
        .width(Length::Fill);

    if bt.powered {
        // Discovery toggle
        content = content.push(discovery_button(bt.discovering));
        content = content.push(separator());

        // Paired devices
        let paired: Vec<_> = bt.devices.iter().filter(|d| d.paired).collect();
        let mut device_list = column![text("Devices")
            .size(style::FONT_SIZE_SMALLER)
            .color(style::M3_ON_SURFACE_VARIANT)]
        .spacing(2.0)
        .width(Length::Fill);

        if paired.is_empty() {
            device_list = device_list.push(
                text("No paired devices")
                    .size(style::FONT_SIZE_NORMAL)
                    .color(style::M3_ON_SURFACE_VARIANT),
            );
        } else {
            for dev in &paired {
                device_list = device_list.push(device_entry(dev));
            }
        }
        content = content.push(device_list);

        // Nearby (unpaired) devices when discovering
        let nearby: Vec<_> = bt.devices.iter().filter(|d| !d.paired).collect();
        if bt.discovering && !nearby.is_empty() {
            let mut nearby_list = column![text("Nearby")
                .size(style::FONT_SIZE_SMALLER)
                .color(style::M3_ON_SURFACE_VARIANT)]
            .spacing(2.0)
            .width(Length::Fill);

            for dev in &nearby {
                nearby_list = nearby_list.push(nearby_entry(dev));
            }
            content = content.push(nearby_list);
        }
    } else {
        content = content.push(
            text("Bluetooth is off")
                .size(style::FONT_SIZE_NORMAL)
                .color(style::M3_ON_SURFACE_VARIANT),
        );
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
