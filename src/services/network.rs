use futures_util::stream::StreamExt;
use futures_util::Stream;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccessPointInfo {
    pub ssid: String,
    pub strength: u8,
    pub icon_name: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(clippy::struct_excessive_bools)]
pub struct NetworkInfo {
    pub connected: bool,
    pub wifi: bool,
    pub wifi_enabled: bool,
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
            wifi_enabled: false,
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
                } else if let Some(existing) = aps.iter_mut().find(|a| a.ssid == ssid) {
                    if strength > existing.strength {
                        existing.strength = strength;
                        existing.icon_name = wifi_icon(strength);
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
    let wifi_enabled: bool = nm_proxy
        .get_property("WirelessEnabled")
        .await
        .unwrap_or(false);

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

    let access_points = if wifi_enabled {
        scan_access_points(conn, &nm_proxy).await
    } else {
        Vec::new()
    };

    NetworkInfo {
        connected,
        wifi,
        wifi_enabled,
        wifi_strength,
        wifi_ssid,
        ethernet,
        icon_name,
        access_points,
    }
}

pub fn set_wifi_enabled(enabled: bool) {
    tokio::spawn(async move {
        let Ok(conn) = zbus::Connection::system().await else {
            return;
        };
        let Some(proxy) = build_proxy(
            &conn,
            "org.freedesktop.NetworkManager",
            "/org/freedesktop/NetworkManager",
            "org.freedesktop.NetworkManager",
        )
        .await
        else {
            return;
        };
        let _ = proxy.set_property("WirelessEnabled", enabled).await;
    });
}

pub fn disconnect_wifi() {
    tokio::spawn(async move {
        let Ok(conn) = zbus::Connection::system().await else {
            return;
        };
        let Some(nm_proxy) = build_proxy(
            &conn,
            "org.freedesktop.NetworkManager",
            "/org/freedesktop/NetworkManager",
            "org.freedesktop.NetworkManager",
        )
        .await
        else {
            return;
        };

        let active_connections: Vec<zbus::zvariant::OwnedObjectPath> = nm_proxy
            .get_property("ActiveConnections")
            .await
            .unwrap_or_default();

        for conn_path in &active_connections {
            let Some(ac_proxy) = build_proxy(
                &conn,
                "org.freedesktop.NetworkManager",
                conn_path.as_str(),
                "org.freedesktop.NetworkManager.Connection.Active",
            )
            .await
            else {
                continue;
            };
            let conn_type: String = ac_proxy.get_property("Type").await.unwrap_or_default();
            if conn_type == "802-11-wireless" {
                let obj_path = zbus::zvariant::ObjectPath::try_from(conn_path.as_str()).ok();
                if let Some(path) = obj_path {
                    let _ = nm_proxy
                        .call_noreply("DeactivateConnection", &(path,))
                        .await;
                }
                break;
            }
        }
    });
}

type ConnSettings = std::collections::HashMap<
    String,
    std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
>;

async fn find_wifi_device_and_ap(
    conn: &zbus::Connection,
    nm_proxy: &zbus::Proxy<'_>,
    ssid: &str,
) -> Option<(String, String)> {
    let devices: Vec<zbus::zvariant::OwnedObjectPath> = nm_proxy
        .get_property("AllDevices")
        .await
        .unwrap_or_default();

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
            if let Some((ap_ssid, _)) = read_ap_info(conn, ap_path.as_str()).await {
                if ap_ssid == ssid {
                    return Some((dev_path.to_string(), ap_path.to_string()));
                }
            }
        }

        // Found a WiFi device but no matching AP — still use this device
        return Some((dev_path.to_string(), "/".to_string()));
    }
    None
}

async fn find_saved_connection(conn: &zbus::Connection, ssid: &str) -> Option<String> {
    let settings_proxy = build_proxy(
        conn,
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager/Settings",
        "org.freedesktop.NetworkManager.Settings",
    )
    .await?;

    let saved_connections: Vec<zbus::zvariant::OwnedObjectPath> = settings_proxy
        .get_property("Connections")
        .await
        .unwrap_or_default();

    for sc_path in &saved_connections {
        let Some(sc_proxy) = build_proxy(
            conn,
            "org.freedesktop.NetworkManager",
            sc_path.as_str(),
            "org.freedesktop.NetworkManager.Settings.Connection",
        )
        .await
        else {
            continue;
        };

        let Ok(settings) = sc_proxy
            .call::<_, _, ConnSettings>("GetSettings", &())
            .await
        else {
            continue;
        };

        if let Some(wifi_settings) = settings.get("802-11-wireless") {
            if let Some(ssid_val) = wifi_settings.get("ssid") {
                if let Ok(ssid_bytes) = <Vec<u8> as TryFrom<_>>::try_from(ssid_val.clone()) {
                    let saved_ssid = String::from_utf8_lossy(&ssid_bytes);
                    if saved_ssid == ssid {
                        return Some(sc_path.to_string());
                    }
                }
            }
        }
    }
    None
}

pub fn connect_network(ssid: String) {
    tokio::spawn(async move {
        let Ok(conn) = zbus::Connection::system().await else {
            return;
        };
        let Some(nm_proxy) = build_proxy(
            &conn,
            "org.freedesktop.NetworkManager",
            "/org/freedesktop/NetworkManager",
            "org.freedesktop.NetworkManager",
        )
        .await
        else {
            return;
        };

        let Some((dev_path, ap_path)) = find_wifi_device_and_ap(&conn, &nm_proxy, &ssid).await
        else {
            return;
        };

        let Some(dev_obj) = zbus::zvariant::ObjectPath::try_from(dev_path.as_str()).ok() else {
            return;
        };
        let Some(ap_obj) = zbus::zvariant::ObjectPath::try_from(ap_path.as_str()).ok() else {
            return;
        };

        if let Some(conn_path) = find_saved_connection(&conn, &ssid).await {
            // Activate existing saved connection
            let Some(conn_obj) = zbus::zvariant::ObjectPath::try_from(conn_path.as_str()).ok()
            else {
                return;
            };
            let _ = nm_proxy
                .call_noreply("ActivateConnection", &(conn_obj, dev_obj, ap_obj))
                .await;
        } else {
            // No saved connection — use AddAndActivateConnection with empty settings.
            // NetworkManager's secret agent (e.g. nm-applet) will prompt for password.
            let empty_settings: std::collections::HashMap<
                &str,
                std::collections::HashMap<&str, zbus::zvariant::Value<'_>>,
            > = std::collections::HashMap::new();
            let _ = nm_proxy
                .call_noreply(
                    "AddAndActivateConnection",
                    &(empty_settings, dev_obj, ap_obj),
                )
                .await;
        }
    });
}

pub fn stream() -> impl Stream<Item = NetworkInfo> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        loop {
            let conn = loop {
                if let Ok(c) = zbus::Connection::system().await {
                    break c;
                }
                log::warn!("network: failed to connect to system D-Bus, retrying");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            };

            if run_network_loop(&conn, &tx).await.is_err() {
                log::warn!("network: signal loop ended, reconnecting");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    });

    tokio_stream::wrappers::UnboundedReceiverStream::new(rx)
}

async fn run_network_loop(
    conn: &zbus::Connection,
    tx: &tokio::sync::mpsc::UnboundedSender<NetworkInfo>,
) -> Result<(), ()> {
    // Subscribe to PropertiesChanged on the main NetworkManager object
    // This fires when ActiveConnections, Connectivity, etc. change
    let nm_props = zbus::fdo::PropertiesProxy::builder(conn)
        .destination("org.freedesktop.NetworkManager")
        .map_err(|_| ())?
        .path("/org/freedesktop/NetworkManager")
        .map_err(|_| ())?
        .build()
        .await
        .map_err(|_| ())?;
    let mut nm_signals = nm_props
        .receive_properties_changed()
        .await
        .map_err(|_| ())?;

    // Subscribe to StateChanged signal for connection state transitions
    let nm_proxy = build_proxy(
        conn,
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager",
        "org.freedesktop.NetworkManager",
    )
    .await
    .ok_or(())?;
    let mut state_changed = nm_proxy
        .receive_signal("StateChanged")
        .await
        .map_err(|_| ())?;

    // Emit initial state
    let mut last = read_network_dbus_with(conn).await;
    tx.send(last.clone()).map_err(|_| ())?;

    loop {
        tokio::select! {
            Some(_) = nm_signals.next() => {}
            Some(_) = state_changed.next() => {}
            // Periodic refresh for AP signal strength updates (not signaled)
            () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
        }

        // Small delay to let D-Bus settle after state changes
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let info = read_network_dbus_with(conn).await;
        if info != last {
            last = info.clone();
            tx.send(info).map_err(|_| ())?;
        }
    }
}
