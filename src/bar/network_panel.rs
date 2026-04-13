use crate::services::network::NetworkInfo;
use crate::style;
use crate::Message;
use iced::widget::{column, container, mouse_area, row, text, Space};
use iced::{Alignment, Border, Element, Length};

const MAX_VISIBLE_NETWORKS: usize = 8;

fn network_entry<'a>(ssid: &'a str, icon_name: &'a str, is_active: bool) -> Element<'a, Message> {
    let (bg, text_color, icon_color) = if is_active {
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

    let wifi_icon = text(icon_name)
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_NORMAL)
        .color(icon_color);

    let label = text(ssid)
        .size(style::FONT_SIZE_NORMAL)
        .color(text_color)
        .width(Length::Fill);

    let content = row![wifi_icon, label]
        .spacing(style::SPACING_SMALLER)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    container(content)
        .padding([style::PADDING_SMALL, style::PADDING_NORMAL])
        .width(Length::Fill)
        .style(move |_theme| container::Style {
            background: Some(iced::Background::Color(bg)),
            border: Border {
                radius: style::ROUNDING_SMALL.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
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

pub fn view(network: &NetworkInfo) -> Element<'_, Message> {
    let header_icon = if network.ethernet {
        style::ICON_CABLE
    } else {
        network.icon_name
    };

    let header = row![
        text(header_icon)
            .font(style::ICON_FONT)
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_PRIMARY),
        text("Network")
            .size(style::FONT_SIZE_LARGE)
            .color(style::M3_ON_SURFACE),
    ]
    .spacing(style::SPACING_SMALLER)
    .align_y(Alignment::Center);

    let mut content = column![header, separator()]
        .spacing(style::SPACING_NORMAL)
        .width(Length::Fill);

    // WiFi network list
    if network.access_points.is_empty() {
        content = content.push(
            text("No Wi-Fi networks found")
                .size(style::FONT_SIZE_NORMAL)
                .color(style::M3_ON_SURFACE_VARIANT),
        );
    } else {
        let mut network_list = column![text("Wi-Fi networks")
            .size(style::FONT_SIZE_SMALLER)
            .color(style::M3_ON_SURFACE_VARIANT)]
        .spacing(2.0)
        .width(Length::Fill);

        let active_ssid = network.wifi_ssid.as_deref();

        // Show active network first, then others sorted by strength (already sorted by service)
        let mut shown = 0;
        if let Some(ssid) = active_ssid {
            if let Some(ap) = network.access_points.iter().find(|a| a.ssid == ssid) {
                network_list = network_list.push(network_entry(&ap.ssid, ap.icon_name, true));
                shown += 1;
            }
        }
        for ap in &network.access_points {
            if shown >= MAX_VISIBLE_NETWORKS {
                break;
            }
            if active_ssid == Some(ap.ssid.as_str()) {
                continue;
            }
            network_list = network_list.push(network_entry(&ap.ssid, ap.icon_name, false));
            shown += 1;
        }

        content = content.push(network_list);
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
            .align_y(Alignment::End),
    )
    .on_exit(Message::CloseAllPanels)
    .into()
}
