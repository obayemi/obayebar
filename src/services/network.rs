use futures_util::Stream;

#[derive(Debug, Clone)]
pub struct AccessPointInfo {
    pub ssid: String,
    pub strength: u8,
    pub icon_name: &'static str,
}

#[derive(Debug, Clone)]
pub struct NetworkInfo {
    pub connected: bool,
    pub wifi: bool,
    pub wifi_strength: u8,
    pub wifi_ssid: Option<String>,
    pub ethernet: bool,
    pub icon_name: &'static str,
    pub access_points: Vec<AccessPointInfo>,
}

impl Default for NetworkInfo {
    fn default() -> Self {
        Self {
            connected: false,
            wifi: false,
            wifi_strength: 0,
            wifi_ssid: None,
            ethernet: false,
            icon_name: crate::style::ICON_WIFI_OFF,
            access_points: Vec::new(),
        }
    }
}

const fn wifi_icon(strength: u8) -> &'static str {
    use crate::style;
    match strength {
        75..=100 => style::ICON_WIFI_4,
        50..=74 => style::ICON_WIFI_3,
        25..=49 => style::ICON_WIFI_2,
        1..=24 => style::ICON_WIFI_1,
        _ => style::ICON_WIFI_0,
    }
}

async fn build_proxy<'a>(
    conn: &'a zbus::Connection,
    dest: &str,
    path: &str,
    iface: &str,
) -> Option<zbus::Proxy<'a>> {
    zbus::proxy::Builder::new(conn)
        .destination(dest.to_string())
        .ok()?
        .path(path.to_string())
        .ok()?
        .interface(iface.to_string())
        .ok()?
        .build()
        .await
        .ok()
}

async fn read_ap_info(conn: &zbus::Connection, ap_path: &str) -> Option<(String, u8)> {
    let ap_proxy = build_proxy(
        conn,
        "org.freedesktop.NetworkManager",
        ap_path,
        "org.freedesktop.NetworkManager.AccessPoint",
    )
    .await?;

    let ssid_bytes: Vec<u8> = ap_proxy.get_property("Ssid").await.ok()?;
    let ssid = String::from_utf8_lossy(&ssid_bytes).into_owned();
    if ssid.is_empty() {
        return None;
    }
    let strength: u8 = ap_proxy.get_property("Strength").await.unwrap_or(0);
    Some((ssid, strength))
}

async fn scan_access_points(
    conn: &zbus::Connection,
    nm_proxy: &zbus::Proxy<'_>,
) -> Vec<AccessPointInfo> {
    let devices: Vec<zbus::zvariant::OwnedObjectPath> = nm_proxy
        .get_property("AllDevices")
        .await
        .unwrap_or_default();

    let mut seen = std::collections::HashSet::new();
    let mut aps = Vec::new();

    for dev_path in &devices {
        let Some(wifi_proxy) = build_proxy(
            conn,
            "org.freedesktop.NetworkManager",
            dev_path.as_str(),
            "org.freedesktop.NetworkManager.Device.Wireless",
        )
        .await
        else {
            continue;
        };

        let ap_paths: Vec<zbus::zvariant::OwnedObjectPath> = wifi_proxy
            .get_property("AccessPoints")
            .await
            .unwrap_or_default();

        for ap_path in &ap_paths {
            if let Some((ssid, strength)) = read_ap_info(conn, ap_path.as_str()).await {
                if seen.insert(ssid.clone()) {
                    aps.push(AccessPointInfo {
                        icon_name: wifi_icon(strength),
                        ssid,
                        strength,
                    });
                } else {
                    // Keep the strongest signal for duplicate SSIDs
                    if let Some(existing) = aps.iter_mut().find(|a| a.ssid == ssid) {
                        if strength > existing.strength {
                            existing.strength = strength;
                            existing.icon_name = wifi_icon(strength);
                        }
                    }
                }
            }
        }
    }

    aps.sort_by_key(|a| std::cmp::Reverse(a.strength));
    aps
}

async fn read_network_dbus_with(conn: &zbus::Connection) -> NetworkInfo {
    let Some(nm_proxy) = build_proxy(
        conn,
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager",
        "org.freedesktop.NetworkManager",
    )
    .await
    else {
        return NetworkInfo::default();
    };

    let connectivity: u32 = nm_proxy.get_property("Connectivity").await.unwrap_or(0);
    let connected = connectivity >= 2;

    let active_connections: Vec<zbus::zvariant::OwnedObjectPath> = nm_proxy
        .get_property("ActiveConnections")
        .await
        .unwrap_or_default();

    let mut wifi = false;
    let mut ethernet = false;
    let mut wifi_strength: u8 = 0;
    let mut wifi_ssid: Option<String> = None;

    for conn_path in &active_connections {
        let Some(ac_proxy) = build_proxy(
            conn,
            "org.freedesktop.NetworkManager",
            conn_path.as_str(),
            "org.freedesktop.NetworkManager.Connection.Active",
        )
        .await
        else {
            continue;
        };

        let conn_type: String = ac_proxy.get_property("Type").await.unwrap_or_default();

        match conn_type.as_str() {
            "802-11-wireless" => {
                wifi = true;
                if let Ok(devices) = ac_proxy
                    .get_property::<Vec<zbus::zvariant::OwnedObjectPath>>("Devices")
                    .await
                {
                    for dev_path in &devices {
                        let Some(dev_proxy) = build_proxy(
                            conn,
                            "org.freedesktop.NetworkManager",
                            dev_path.as_str(),
                            "org.freedesktop.NetworkManager.Device.Wireless",
                        )
                        .await
                        else {
                            continue;
                        };

                        if let Ok(ap_path) = dev_proxy
                            .get_property::<zbus::zvariant::OwnedObjectPath>("ActiveAccessPoint")
                            .await
                        {
                            if let Some((ssid, strength)) =
                                read_ap_info(conn, ap_path.as_str()).await
                            {
                                wifi_ssid = Some(ssid);
                                wifi_strength = strength;
                            }
                        }
                    }
                }
            }
            "802-3-ethernet" => {
                ethernet = true;
            }
            _ => {}
        }
    }

    let icon_name = if ethernet {
        crate::style::ICON_CABLE
    } else if wifi {
        wifi_icon(wifi_strength)
    } else if connected {
        crate::style::ICON_LANGUAGE
    } else {
        crate::style::ICON_WIFI_OFF
    };

    let access_points = scan_access_points(conn, &nm_proxy).await;

    NetworkInfo {
        connected,
        wifi,
        wifi_strength,
        wifi_ssid,
        ethernet,
        icon_name,
        access_points,
    }
}

pub fn stream() -> impl Stream<Item = NetworkInfo> {
    futures_util::stream::unfold(
        (None, false),
        |(conn, should_sleep): (Option<zbus::Connection>, bool)| async move {
            if should_sleep {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
            let connection = if let Some(c) = conn {
                c
            } else if let Ok(c) = zbus::Connection::system().await {
                c
            } else {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                return Some((NetworkInfo::default(), (None, false)));
            };
            let info = read_network_dbus_with(&connection).await;
            Some((info, (Some(connection), true)))
        },
    )
}
