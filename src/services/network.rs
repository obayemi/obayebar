use futures_util::Stream;

#[derive(Debug, Clone)]
pub struct NetworkInfo {
    pub connected: bool,
    pub wifi: bool,
    pub wifi_strength: u8,
    pub ethernet: bool,
    pub icon_name: &'static str,
}

impl Default for NetworkInfo {
    fn default() -> Self {
        Self {
            connected: false,
            wifi: false,
            wifi_strength: 0,
            ethernet: false,
            icon_name: crate::style::ICON_WIFI_OFF,
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
                            let Some(access_point_proxy) = build_proxy(
                                conn,
                                "org.freedesktop.NetworkManager",
                                ap_path.as_str(),
                                "org.freedesktop.NetworkManager.AccessPoint",
                            )
                            .await
                            else {
                                continue;
                            };

                            wifi_strength = access_point_proxy
                                .get_property::<u8>("Strength")
                                .await
                                .unwrap_or(0);
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

    NetworkInfo {
        connected,
        wifi,
        wifi_strength,
        ethernet,
        icon_name,
    }
}

pub fn stream() -> impl Stream<Item = NetworkInfo> {
    futures_util::stream::unfold(None, |conn: Option<zbus::Connection>| async {
        let connection = if let Some(c) = conn {
            c
        } else if let Ok(c) = zbus::Connection::system().await {
            c
        } else {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            return Some((NetworkInfo::default(), None));
        };
        let info = read_network_dbus_with(&connection).await;
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        Some((info, Some(connection)))
    })
}
