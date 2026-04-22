use super::widgets::{icon_button, panel_with_exit, separator, styled_toggler};
use crate::services::network::NetworkInfo;
use crate::Message;
use iced::widget::{column, container, row, text, Space};
use iced::{Alignment, Border, Element, Length};
use obayebar::style;

const MAX_VISIBLE_NETWORKS: usize = 8;

fn network_entry<'a>(
    ssid: &'a str,
    icon_name: &'a str,
    is_active: bool,
    is_connecting: bool,
) -> Element<'a, Message> {
    let (bg, text_color, icon_color) = if is_active {
        (
            style::with_alpha(style::M3_PRIMARY, 0.15),
            style::M3_PRIMARY,
            style::M3_PRIMARY,
        )
    } else if is_connecting {
        (
            style::with_alpha(style::M3_TERTIARY, 0.10),
            style::M3_TERTIARY,
            style::M3_TERTIARY,
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

    let mut label_row = row![text(ssid).size(style::FONT_SIZE_NORMAL).color(text_color)]
        .spacing(style::SPACING_SMALLER)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    if is_connecting {
        label_row = label_row.push(
            text(style::ICON_AUTORENEW)
                .font(style::ICON_FONT)
                .size(style::FONT_SIZE_SMALL)
                .color(style::M3_TERTIARY),
        );
    }

    let action: Element<'a, Message> = if is_connecting {
        // No action button while connecting
        Space::new().width(0.0).into()
    } else {
        let (action_icon, action_msg) = if is_active {
            (style::ICON_CLOSE, Message::NetworkDisconnect)
        } else {
            (
                style::ICON_WIFI_4,
                Message::NetworkConnect(ssid.to_string()),
            )
        };
        icon_button(action_icon, style::M3_ON_SURFACE_VARIANT, action_msg)
    };

    let content = row![wifi_icon, label_row, action]
        .spacing(style::SPACING_SMALLER)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    container(content)
        .padding(style::PADDING_ENTRY)
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

fn active_connection_entry<'a>(name: &'a str, icon_name: &'a str) -> Element<'a, Message> {
    let icon = text(icon_name)
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_NORMAL)
        .color(style::M3_PRIMARY);

    let label = text(name)
        .size(style::FONT_SIZE_NORMAL)
        .color(style::M3_PRIMARY)
        .width(Length::Fill);

    let content = row![icon, label]
        .spacing(style::SPACING_SMALLER)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    container(content)
        .padding(style::PADDING_ENTRY)
        .width(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(iced::Background::Color(style::with_alpha(
                style::M3_PRIMARY,
                0.15,
            ))),
            border: Border {
                radius: style::ROUNDING_SMALL.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

fn connection_type_label(conn_type: &str) -> &'static str {
    match conn_type {
        "802-3-ethernet" => "Ethernet",
        "wireguard" => "Wireguard",
        "vpn" => "VPN",
        "bridge" => "Bridge",
        "bond" => "Bond",
        _ => "Other",
    }
}

#[allow(clippy::too_many_lines)]
pub fn view<'a>(
    network: &'a NetworkInfo,
    connecting_ssid: Option<&'a str>,
) -> Element<'a, Message> {
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
        Space::new().width(Length::Fill),
        styled_toggler(network.wifi_enabled, Message::NetworkSetWifiEnabled),
    ]
    .spacing(style::SPACING_SMALLER)
    .align_y(Alignment::Center);

    let mut content = column![header, separator()]
        .spacing(style::SPACING_NORMAL)
        .width(Length::Fill);

    // Active wired / VPN / wireguard connections, grouped by type
    if !network.active_connections.is_empty() {
        let mut groups: Vec<(&str, Vec<&crate::services::network::ActiveConnectionInfo>)> =
            Vec::new();
        for ac in &network.active_connections {
            if let Some(group) = groups.iter_mut().find(|(t, _)| *t == ac.conn_type) {
                group.1.push(ac);
            } else {
                groups.push((&ac.conn_type, vec![ac]));
            }
        }

        for (conn_type, conns) in &groups {
            let label = connection_type_label(conn_type);
            let mut section = column![text(label)
                .size(style::FONT_SIZE_SMALLER)
                .color(style::M3_ON_SURFACE_VARIANT)]
            .spacing(2.0)
            .width(Length::Fill);

            for ac in conns {
                section = section.push(active_connection_entry(&ac.name, ac.icon_name));
            }

            content = content.push(section);
        }
        content = content.push(separator());
    }

    if network.wifi_enabled {
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

            // Show connecting network first, then active, then others
            let mut shown = 0;

            // Connecting network at top (if not already active)
            if let Some(c_ssid) = connecting_ssid {
                if active_ssid != Some(c_ssid) {
                    if let Some(ap) = network.access_points.iter().find(|a| a.ssid == c_ssid) {
                        network_list =
                            network_list.push(network_entry(&ap.ssid, ap.icon_name, false, true));
                        shown += 1;
                    }
                }
            }

            // Active network
            if let Some(ssid) = active_ssid {
                if let Some(ap) = network.access_points.iter().find(|a| a.ssid == ssid) {
                    network_list =
                        network_list.push(network_entry(&ap.ssid, ap.icon_name, true, false));
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
                if connecting_ssid == Some(ap.ssid.as_str()) {
                    continue;
                }
                network_list =
                    network_list.push(network_entry(&ap.ssid, ap.icon_name, false, false));
                shown += 1;
            }

            content = content.push(network_list);
        }
    } else {
        content = content.push(
            text("Wi-Fi is off")
                .size(style::FONT_SIZE_NORMAL)
                .color(style::M3_ON_SURFACE_VARIANT),
        );
    }

    let panel = container(content)
        .padding(style::PADDING_LARGE)
        .width(Length::Fill)
        .height(Length::Shrink)
        .style(style::audio_panel_container);

    panel_with_exit(panel.into())
}
