use crate::services::dbus_util::{self, PanelSignal};
use futures_util::stream::StreamExt;
use futures_util::Stream;

static PANEL: PanelSignal = PanelSignal::new();

/// Toggle from the UI when the bluetooth panel opens/closes.
pub fn set_panel_open(open: bool) {
    PANEL.set(open);
}

const BLUEZ: &str = "org.bluez";
const ADAPTER1: &str = "org.bluez.Adapter1";
const DEVICE1: &str = "org.bluez.Device1";
const BATTERY1: &str = "org.bluez.Battery1";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";

/// Build an `org.bluez` proxy. Local wrapper around `dbus_util::proxy` that
/// pins the destination, since every call in this module targets `BlueZ`.
async fn build_proxy<'a>(
    conn: &'a zbus::Connection,
    path: &str,
    iface: &str,
) -> Option<zbus::Proxy<'a>> {
    dbus_util::proxy(conn, BLUEZ, path, iface).await
}

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
    pub discovering: bool,
    pub icon_name: &'static str,
    pub devices: Vec<BluetoothDevice>,
}

impl Default for BluetoothInfo {
    fn default() -> Self {
        Self {
            powered: false,
            discovering: false,
            icon_name: obayebar::style::ICON_BLUETOOTH_DISABLED,
            devices: Vec::new(),
        }
    }
}

const fn bt_icon(powered: bool, has_connected: bool) -> &'static str {
    use obayebar::style;
    if !powered {
        style::ICON_BLUETOOTH_DISABLED
    } else if has_connected {
        style::ICON_BLUETOOTH_CONNECTED
    } else {
        style::ICON_BLUETOOTH
    }
}

/// Choose one adapter path from those `BlueZ` exposes.
///
/// Sorted so the choice is stable across refreshes on a machine with more than
/// one adapter — an unstable pick would make the panel flip between adapters.
fn pick_adapter_path<I: IntoIterator<Item = String>>(paths: I) -> Option<String> {
    let mut paths: Vec<String> = paths.into_iter().collect();
    paths.sort();
    paths.into_iter().next()
}

/// Find the `BlueZ` adapter's object path in the exported object tree.
///
/// The path used to be hardcoded to `/org/bluez/hci0`. On a machine whose
/// adapter is `hci1` that path does not exist, but zbus proxies are built
/// lazily — so construction succeeded, only the property read failed, and
/// `unwrap_or(false)` reported "Bluetooth is off" forever with the power
/// toggle writing into the void. The object tree is already fetched for device
/// enumeration, so the real path costs nothing extra to find.
async fn find_adapter_path(conn: &zbus::Connection) -> Option<String> {
    let om_proxy = build_proxy(conn, "/", OBJECT_MANAGER).await?;
    let objects = om_proxy
        .call::<_, _, ManagedObjects>("GetManagedObjects", &())
        .await
        .map_err(|e| log::warn!("bluetooth: GetManagedObjects failed: {e}"))
        .ok()?;
    pick_adapter_path(
        objects
            .iter()
            .filter(|(_, ifaces)| ifaces.contains_key(ADAPTER1))
            .map(|(path, _)| path.to_string()),
    )
}

async fn read_bluetooth_dbus(conn: &zbus::Connection, adapter_path: Option<&str>) -> BluetoothInfo {
    let Some(adapter_path) = adapter_path else {
        // No `Adapter1` anywhere in the tree: no hardware, or bluetoothd is
        // down. Render as off — there is nothing to toggle — but deliberately
        // do not fail the loop: forcing a reconnect here would log-spam every
        // five seconds on a machine with no Bluetooth at all.
        return BluetoothInfo::default();
    };
    let Some(adapter) = build_proxy(conn, adapter_path, ADAPTER1).await else {
        log::warn!("bluetooth: could not build adapter proxy for {adapter_path}");
        return BluetoothInfo::default();
    };

    let powered: bool = match adapter.get_property("Powered").await {
        Ok(powered) => powered,
        Err(e) => {
            // Distinct from "the adapter says it is off", which is why this is
            // logged rather than collapsed into `unwrap_or(false)`.
            log::warn!("bluetooth: reading Powered on {adapter_path} failed: {e}");
            return BluetoothInfo::default();
        }
    };
    if !powered {
        return BluetoothInfo {
            powered: false,
            discovering: false,
            icon_name: obayebar::style::ICON_BLUETOOTH_DISABLED,
            devices: Vec::new(),
        };
    }

    let discovering: bool = adapter.get_property("Discovering").await.unwrap_or(false);

    // The device list is populated whether or not the panel is open. The old
    // "cheap path" for the closed case still made the same
    // `GetManagedObjects` call and only skipped per-device deserialization, so
    // it saved very little — while guaranteeing the panel was sized from an
    // empty list, since the open path measures before flipping the signal.
    let devices = enumerate_devices(conn, discovering).await;
    let has_connected = devices.iter().any(|d| d.connected);
    BluetoothInfo {
        powered: true,
        discovering,
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

async fn enumerate_devices(
    conn: &zbus::Connection,
    include_unpaired: bool,
) -> Vec<BluetoothDevice> {
    let Some(om_proxy) = build_proxy(conn, "/", OBJECT_MANAGER).await else {
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
        let Some(dev_props) = ifaces.get(DEVICE1) else {
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

        if !paired && !include_unpaired {
            continue;
        }

        let battery = ifaces
            .get(BATTERY1)
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

/// Connect to the system bus for a one-shot action, reporting why if it fails.
async fn system_bus(action: &str) -> Option<zbus::Connection> {
    match zbus::Connection::system().await {
        Ok(conn) => Some(conn),
        Err(e) => {
            log::warn!("bluetooth: cannot {action}, no system bus: {e}");
            None
        }
    }
}

/// Resolve the adapter proxy for a one-shot action, reporting why if it fails.
async fn adapter_for<'a>(conn: &'a zbus::Connection, action: &str) -> Option<zbus::Proxy<'a>> {
    let Some(path) = find_adapter_path(conn).await else {
        log::warn!("bluetooth: cannot {action}, no {ADAPTER1} found");
        return None;
    };
    let proxy = build_proxy(conn, &path, ADAPTER1).await;
    if proxy.is_none() {
        log::warn!("bluetooth: cannot {action}, no proxy for {path}");
    }
    proxy
}

/// Report the outcome of a `BlueZ` call instead of discarding it.
///
/// These used to go through `call_noreply`, whose `NO_REPLY_EXPECTED` flag
/// means `BlueZ` never sends its error reply at all — so even a correct `if let
/// Err` could not have seen `org.bluez.Error.Failed` or
/// `br-connection-profile-unavailable`. A failed action was a silent no-op:
/// the user re-clicked blindly and there was nothing at any log level.
fn log_result<E: std::fmt::Display>(action: &str, result: Result<(), E>) {
    match result {
        Ok(()) => log::debug!("bluetooth: {action} succeeded"),
        Err(e) => log::warn!("bluetooth: {action} failed: {e}"),
    }
}

pub fn toggle_device_connection(path: &str, currently_connected: bool) {
    let path = path.to_string();
    tokio::spawn(async move {
        let method = if currently_connected {
            "Disconnect"
        } else {
            "Connect"
        };
        let action = format!("{method} {path}");
        let Some(conn) = system_bus(&action).await else {
            return;
        };
        let Some(proxy) = build_proxy(&conn, &path, DEVICE1).await else {
            log::warn!("bluetooth: cannot {action}, no proxy for {path}");
            return;
        };
        // A replying call, so BlueZ's error actually reaches us. `Connect` can
        // outlast zbus's default 25s reply timeout on a device that is powered
        // off — that surfaces here as a timeout error, which is still a far
        // better signal than the previous silence.
        log_result(&action, proxy.call::<_, _, ()>(method, &()).await);
    });
}

pub fn set_adapter_powered(powered: bool) {
    tokio::spawn(async move {
        let action = if powered { "power on" } else { "power off" };
        let Some(conn) = system_bus(action).await else {
            return;
        };
        let Some(proxy) = adapter_for(&conn, action).await else {
            return;
        };
        log_result(action, proxy.set_property("Powered", powered).await);
    });
}

pub fn set_discovery(active: bool) {
    tokio::spawn(async move {
        let method = if active {
            "StartDiscovery"
        } else {
            "StopDiscovery"
        };
        let Some(conn) = system_bus(method).await else {
            return;
        };
        let Some(proxy) = adapter_for(&conn, method).await else {
            return;
        };
        log_result(method, proxy.call::<_, _, ()>(method, &()).await);
    });
}

pub fn remove_device(device_path: &str) {
    let device_path = device_path.to_string();
    tokio::spawn(async move {
        let action = format!("RemoveDevice {device_path}");
        let Some(conn) = system_bus(&action).await else {
            return;
        };
        let Some(proxy) = adapter_for(&conn, &action).await else {
            return;
        };
        let Ok(path) = zbus::zvariant::ObjectPath::try_from(device_path.as_str()) else {
            log::warn!("bluetooth: cannot {action}, not a valid object path");
            return;
        };
        log_result(
            &action,
            proxy.call::<_, _, ()>("RemoveDevice", &(path,)).await,
        );
    });
}

pub fn stream() -> impl Stream<Item = BluetoothInfo> {
    dbus_util::spawn_stream(
        "bluetooth",
        dbus_util::Bus::System,
        std::time::Duration::from_secs(5),
        |conn, tx| async move { run_bluetooth_loop(&conn, &tx).await },
    )
}

async fn run_bluetooth_loop(
    conn: &zbus::Connection,
    tx: &tokio::sync::mpsc::UnboundedSender<BluetoothInfo>,
) -> Result<(), ()> {
    // Subscribe to ObjectManager signals for device add/remove. These also
    // fire when an adapter itself appears or vanishes, which is what lets the
    // adapter-change check at the bottom of the loop notice a dongle swap.
    let om_proxy = build_proxy(conn, "/", OBJECT_MANAGER).await.ok_or(())?;
    let mut ifaces_added = om_proxy
        .receive_signal("InterfacesAdded")
        .await
        .map_err(|_| ())?;
    let mut ifaces_removed = om_proxy
        .receive_signal("InterfacesRemoved")
        .await
        .map_err(|_| ())?;

    // Subscribe to PropertiesChanged on whichever adapter actually exists.
    // With no adapter there is nothing to subscribe to, so that arm becomes a
    // stream that never yields rather than a subscription to a path that
    // silently accepts AddMatch and then never fires.
    let adapter_path = find_adapter_path(conn).await;
    if adapter_path.is_none() {
        log::info!("bluetooth: no {ADAPTER1} present; reporting Bluetooth as off");
    }
    let adapter_props = match adapter_path.as_deref() {
        Some(path) => Some(
            dbus_util::properties_proxy(conn, BLUEZ, path)
                .await
                .ok_or(())?,
        ),
        None => None,
    };
    let mut adapter_signals: std::pin::Pin<Box<dyn Stream<Item = ()> + Send>> =
        match adapter_props.as_ref() {
            Some(props) => props
                .receive_properties_changed()
                .await
                .map_err(|_| ())?
                .map(|_| ())
                .boxed(),
            None => futures_util::stream::pending().boxed(),
        };

    // Emit initial state
    let mut last = read_bluetooth_dbus(conn, adapter_path.as_deref()).await;
    tx.send(last.clone()).map_err(|_| ())?;

    loop {
        tokio::select! {
            Some(_) = ifaces_added.next() => {}
            Some(_) = ifaces_removed.next() => {}
            Some(()) = adapter_signals.next() => {}
            () = PANEL.changed() => {}
            // Fallback refresh every 2 minutes
            () = tokio::time::sleep(std::time::Duration::from_mins(2)) => {}
        }

        // Small delay to let D-Bus settle
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // An adapter appearing, vanishing or being renumbered means our
        // PropertiesChanged subscription is pointed at the wrong path.
        // Returning `Ok` makes `spawn_stream` re-enter immediately — no
        // reconnect delay and no warning, unlike the `Err` path — so we come
        // straight back with the right subscription. On a machine with no
        // Bluetooth this never fires, so there is nothing to spam.
        let current = find_adapter_path(conn).await;
        if current != adapter_path {
            log::info!("bluetooth: adapter changed {adapter_path:?} -> {current:?}, resubscribing");
            return Ok(());
        }

        let info = read_bluetooth_dbus(conn, adapter_path.as_deref()).await;
        dbus_util::send_if_changed(tx, &mut last, info)?;
    }
}

#[cfg(test)]
mod tests {
    use super::pick_adapter_path;

    #[test]
    fn no_adapter_yields_none() {
        assert_eq!(pick_adapter_path(Vec::new()), None);
    }

    #[test]
    fn picks_the_only_adapter_whatever_its_number() {
        // The path used to be hardcoded to hci0, so an hci1-only machine
        // reported "Bluetooth off" forever.
        assert_eq!(
            pick_adapter_path(vec!["/org/bluez/hci1".to_string()]),
            Some("/org/bluez/hci1".to_string())
        );
    }

    #[test]
    fn choice_is_stable_across_input_order() {
        // GetManagedObjects returns a HashMap, so iteration order varies; an
        // unstable pick would flip the panel between adapters on refresh.
        let a = pick_adapter_path(vec![
            "/org/bluez/hci2".to_string(),
            "/org/bluez/hci0".to_string(),
            "/org/bluez/hci1".to_string(),
        ]);
        let b = pick_adapter_path(vec![
            "/org/bluez/hci1".to_string(),
            "/org/bluez/hci2".to_string(),
            "/org/bluez/hci0".to_string(),
        ]);
        assert_eq!(a, b);
        assert_eq!(a, Some("/org/bluez/hci0".to_string()));
    }
}
