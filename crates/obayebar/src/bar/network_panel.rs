use super::widgets::{icon_button, panel_body, panel_header, separator, styled_toggler};
use crate::panel::PanelKind;
use crate::services::network::{AccessPointInfo, NetworkInfo};
use crate::style;
use crate::Message;
use iced::widget::{column, container, row, text, Space};
use iced::{Alignment, Border, Element, Length};

fn network_entry(row: WifiRow<'_>) -> Element<'_, Message> {
    let ssid = row.ap.ssid.as_str();
    let icon_name = row.ap.icon_name;

    let (bg, text_color, icon_color, show_spinner, action) = match row.state {
        WifiRowState::Active => (
            style::with_alpha(style::M3_PRIMARY, 0.15),
            style::M3_PRIMARY,
            style::M3_PRIMARY,
            false,
            icon_button(
                style::ICON_CLOSE,
                style::M3_ON_SURFACE_VARIANT,
                Message::NetworkDisconnect,
            ),
        ),
        WifiRowState::Connecting => (
            style::with_alpha(style::M3_TERTIARY, 0.10),
            style::M3_TERTIARY,
            style::M3_TERTIARY,
            true,
            Space::new().width(0.0).into(),
        ),
        WifiRowState::Idle => (
            iced::Color::TRANSPARENT,
            style::M3_ON_SURFACE,
            style::M3_ON_SURFACE_VARIANT,
            false,
            icon_button(
                style::ICON_WIFI_4,
                style::M3_ON_SURFACE_VARIANT,
                Message::NetworkConnect(ssid.to_string()),
            ),
        ),
    };

    let wifi_icon = text(icon_name)
        .font(style::ICON_FONT)
        .size(style::FONT_SIZE_NORMAL)
        .color(icon_color);

    let mut label_row = row![text(ssid).size(style::FONT_SIZE_NORMAL).color(text_color)]
        .spacing(style::SPACING_SMALLER)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    if show_spinner {
        label_row = label_row.push(
            text(style::ICON_AUTORENEW)
                .font(style::ICON_FONT)
                .size(style::FONT_SIZE_SMALL)
                .color(style::M3_TERTIARY),
        );
    }

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

/// Which of the mutually exclusive states a Wi-Fi row is rendered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WifiRowState {
    Connecting,
    Active,
    Idle,
}

#[derive(Clone, Copy)]
struct WifiRow<'a> {
    ap: &'a AccessPointInfo,
    state: WifiRowState,
}

/// Which access points to render, and in what order: the connecting
/// network first (unless it is already the active one), then the active
/// network, then the rest in their given order until `max_visible` rows
/// are shown; the connecting and active rows are always included.
fn select_wifi_rows<'a>(
    access_points: &'a [AccessPointInfo],
    active_ssid: Option<&str>,
    connecting_ssid: Option<&str>,
    max_visible: usize,
) -> Vec<WifiRow<'a>> {
    let pinned = [
        (
            connecting_ssid.filter(|c| active_ssid != Some(*c)),
            WifiRowState::Connecting,
        ),
        (active_ssid, WifiRowState::Active),
    ];
    let mut rows: Vec<WifiRow<'a>> = pinned
        .into_iter()
        .filter_map(|(ssid, state)| {
            let ap = access_points
                .iter()
                .find(|a| Some(a.ssid.as_str()) == ssid)?;
            Some(WifiRow { ap, state })
        })
        .collect();

    for ap in access_points {
        if rows.len() >= max_visible {
            break;
        }
        if active_ssid == Some(ap.ssid.as_str()) || connecting_ssid == Some(ap.ssid.as_str()) {
            continue;
        }
        rows.push(WifiRow {
            ap,
            state: WifiRowState::Idle,
        });
    }

    rows
}

pub fn view<'a>(
    network: &'a NetworkInfo,
    connecting_ssid: Option<&'a str>,
) -> Element<'a, Message> {
    let header_icon = if network.ethernet {
        style::ICON_CABLE
    } else {
        network.icon_name
    };

    let header = panel_header(header_icon, "Network", style::M3_PRIMARY)
        .push(Space::new().width(Length::Fill))
        .push(styled_toggler(
            network.wifi_enabled,
            Message::NetworkSetWifiEnabled,
        ));

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

            let rows = select_wifi_rows(
                &network.access_points,
                network.wifi_ssid.as_deref(),
                connecting_ssid,
                style::PANEL_MAX_VISIBLE_ROWS,
            );
            for row in rows {
                network_list = network_list.push(network_entry(row));
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

    panel_body(PanelKind::Network, content)
}

#[cfg(test)]
mod select_wifi_rows_tests {
    use super::{select_wifi_rows, AccessPointInfo};

    fn ap(ssid: &str) -> AccessPointInfo {
        AccessPointInfo {
            ssid: ssid.to_string(),
            strength: 50,
            icon_name: "icon",
            known: true,
        }
    }

    #[test]
    fn empty_access_points_yields_no_rows() {
        let rows = select_wifi_rows(&[], None, None, 8);
        assert!(rows.is_empty());
    }

    fn states<'a>(rows: &[super::WifiRow<'a>]) -> Vec<(&'a str, super::WifiRowState)> {
        rows.iter().map(|r| (r.ap.ssid.as_str(), r.state)).collect()
    }

    #[test]
    fn connecting_then_active_then_rest_in_order() {
        use super::WifiRowState::{Active, Connecting, Idle};
        let aps = [ap("a"), ap("b"), ap("c")];
        let rows = select_wifi_rows(&aps, Some("b"), Some("c"), 8);
        assert_eq!(
            states(&rows),
            vec![("c", Connecting), ("b", Active), ("a", Idle)]
        );
    }

    #[test]
    fn active_ssid_also_connecting_appears_once_as_active() {
        use super::WifiRowState::{Active, Idle};
        let aps = [ap("a"), ap("b")];
        let rows = select_wifi_rows(&aps, Some("a"), Some("a"), 8);
        assert_eq!(states(&rows), vec![("a", Active), ("b", Idle)]);
    }

    #[test]
    fn caps_at_max_visible_rows() {
        let aps = [ap("a"), ap("b"), ap("c")];
        let rows = select_wifi_rows(&aps, None, None, 2);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn zero_max_visible_shows_nothing_from_the_tail_loop() {
        let aps = [ap("a"), ap("b")];
        let rows = select_wifi_rows(&aps, None, None, 0);
        assert!(rows.is_empty());
    }

    #[test]
    fn active_and_connecting_rows_are_shown_even_past_the_cap() {
        let aps = [ap("a"), ap("b"), ap("c")];
        let rows = select_wifi_rows(&aps, Some("a"), Some("b"), 1);
        let ssids: Vec<&str> = rows.iter().map(|r| r.ap.ssid.as_str()).collect();
        assert_eq!(ssids, ["b", "a"]);
    }
}
