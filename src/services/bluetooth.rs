use futures_util::stream::StreamExt;
use futures_util::Stream;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BluetoothDevice {
    pub alias: String,
    pub icon: String,
    pub connected: bool,
    pub paired: bool,
    pub battery: Option<u8>,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BluetoothInfo {
    pub powered: bool,
    pub icon_name: &'static str,
    pub devices: Vec<BluetoothDevice>,
}

impl Default for BluetoothInfo {
    fn default() -> Self {
        Self {
            powered: false,
            icon_name: crate::style::ICON_BLUETOOTH_DISABLED,
            devices: Vec::new(),
        }
    }
}

const fn bt_icon(powered: bool, has_connected: bool) -> &'static str {
    use crate::style;
    if !powered {
        style::ICON_BLUETOOTH_DISABLED
    } else if has_connected {
        style::ICON_BLUETOOTH_CONNECTED
    } else {
        style::ICON_BLUETOOTH
    }
}

async fn build_proxy<'a>(
    conn: &'a zbus::Connection,
    path: &str,
    iface: &str,
) -> Option<zbus::Proxy<'a>> {
    zbus::proxy::Builder::new(conn)
        .destination("org.bluez")
        .ok()?
        .path(path.to_string())
        .ok()?
        .interface(iface.to_string())
        .ok()?
        .build()
        .await
        .ok()
}

async fn read_bluetooth_dbus(conn: &zbus::Connection) -> BluetoothInfo {
    let Some(adapter) = build_proxy(conn, "/org/bluez/hci0", "org.bluez.Adapter1").await else {
        return BluetoothInfo::default();
    };

    let powered: bool = adapter.get_property("Powered").await.unwrap_or(false);
    if !powered {
        return BluetoothInfo {
            powered: false,
            icon_name: crate::style::ICON_BLUETOOTH_DISABLED,
            devices: Vec::new(),
        };
    }

    let devices = enumerate_devices(conn).await;
    let has_connected = devices.iter().any(|d| d.connected);

    BluetoothInfo {
        powered: true,
        icon_name: bt_icon(true, has_connected),
        devices,
    }
}

type ManagedObjects = std::collections::HashMap<
    zbus::zvariant::OwnedObjectPath,
    std::collections::HashMap<
        String,
        std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
    >,
>;

async fn enumerate_devices(conn: &zbus::Connection) -> Vec<BluetoothDevice> {
    let Some(om_proxy) = build_proxy(conn, "/", "org.freedesktop.DBus.ObjectManager").await else {
        return Vec::new();
    };

    let Ok(objects) = om_proxy
        .call::<_, _, ManagedObjects>("GetManagedObjects", &())
        .await
    else {
        return Vec::new();
    };

    let mut devices = Vec::new();

    for (path, ifaces) in &objects {
        let Some(dev_props) = ifaces.get("org.bluez.Device1") else {
            continue;
        };

        let alias = dev_props
            .get("Alias")
            .and_then(|v| <String as TryFrom<_>>::try_from(v.clone()).ok())
            .unwrap_or_default();

        let icon = dev_props
            .get("Icon")
            .and_then(|v| <String as TryFrom<_>>::try_from(v.clone()).ok())
            .unwrap_or_default();

        let connected = dev_props
            .get("Connected")
            .and_then(|v| <bool as TryFrom<_>>::try_from(v.clone()).ok())
            .unwrap_or(false);

        let paired = dev_props
            .get("Paired")
            .and_then(|v| <bool as TryFrom<_>>::try_from(v.clone()).ok())
            .unwrap_or(false);

        if !paired {
            continue;
        }

        let battery = ifaces
            .get("org.bluez.Battery1")
            .and_then(|bat_props| bat_props.get("Percentage"))
            .and_then(|v| <u8 as TryFrom<_>>::try_from(v.clone()).ok());

        devices.push(BluetoothDevice {
            alias,
            icon,
            connected,
            paired,
            battery,
            path: path.to_string(),
        });
    }

    devices.sort_by(|a, b| b.connected.cmp(&a.connected).then(a.alias.cmp(&b.alias)));
    devices
}

pub fn toggle_device_connection(path: &str, currently_connected: bool) {
    let path = path.to_string();
    tokio::spawn(async move {
        let Ok(conn) = zbus::Connection::system().await else {
            return;
        };
        let Some(proxy) = build_proxy(&conn, &path, "org.bluez.Device1").await else {
            return;
        };
        let method = if currently_connected {
            "Disconnect"
        } else {
            "Connect"
        };
        let _ = proxy.call_noreply(method, &()).await;
    });
}

pub fn stream() -> impl Stream<Item = BluetoothInfo> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        loop {
            let conn = loop {
                if let Ok(c) = zbus::Connection::system().await {
                    break c;
                }
                log::warn!("bluetooth: failed to connect to system D-Bus, retrying");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            };

            if run_bluetooth_loop(&conn, &tx).await.is_err() {
                log::warn!("bluetooth: signal loop ended, reconnecting");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    });

    tokio_stream::wrappers::UnboundedReceiverStream::new(rx)
}

async fn run_bluetooth_loop(
    conn: &zbus::Connection,
    tx: &tokio::sync::mpsc::UnboundedSender<BluetoothInfo>,
) -> Result<(), ()> {
    // Subscribe to ObjectManager signals for device add/remove
    let om_proxy = build_proxy(conn, "/", "org.freedesktop.DBus.ObjectManager")
        .await
        .ok_or(())?;
    let mut ifaces_added = om_proxy
        .receive_signal("InterfacesAdded")
        .await
        .map_err(|_| ())?;
    let mut ifaces_removed = om_proxy
        .receive_signal("InterfacesRemoved")
        .await
        .map_err(|_| ())?;

    // Subscribe to PropertiesChanged on the adapter for Powered state
    let adapter_props = zbus::fdo::PropertiesProxy::builder(conn)
        .destination("org.bluez")
        .map_err(|_| ())?
        .path("/org/bluez/hci0")
        .map_err(|_| ())?
        .build()
        .await
        .map_err(|_| ())?;
    let mut adapter_signals = adapter_props
        .receive_properties_changed()
        .await
        .map_err(|_| ())?;

    // Emit initial state
    let mut last = read_bluetooth_dbus(conn).await;
    tx.send(last.clone()).map_err(|_| ())?;

    loop {
        tokio::select! {
            Some(_) = ifaces_added.next() => {}
            Some(_) = ifaces_removed.next() => {}
            Some(_) = adapter_signals.next() => {}
            // Fallback refresh every 2 minutes
            () = tokio::time::sleep(std::time::Duration::from_mins(2)) => {}
        }

        // Small delay to let D-Bus settle
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let info = read_bluetooth_dbus(conn).await;
        if info != last {
            last = info.clone();
            tx.send(info).map_err(|_| ())?;
        }
    }
}
