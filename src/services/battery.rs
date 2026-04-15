use std::sync::OnceLock;

use futures_util::stream::StreamExt;
use futures_util::Stream;
use tokio::sync::Notify;

static REFRESH_NOTIFY: OnceLock<Notify> = OnceLock::new();

fn refresh_notify() -> &'static Notify {
    REFRESH_NOTIFY.get_or_init(Notify::new)
}

#[derive(Debug, Clone)]
pub struct PowerProfileInfo {
    pub available_profiles: Vec<String>,
    pub active_profile: String,
}

#[derive(Debug, Clone)]
pub struct BatteryInfo {
    pub present: bool,
    pub percentage: f64,
    pub charging: bool,
    pub icon_name: &'static str,
    /// Seconds until empty (on battery), 0 if unknown
    pub time_to_empty: i64,
    /// Seconds until full (on AC), 0 if unknown
    pub time_to_full: i64,
    pub power_profiles: Option<PowerProfileInfo>,
}

impl Default for BatteryInfo {
    fn default() -> Self {
        Self {
            present: false,
            percentage: 100.0,
            charging: false,
            icon_name: crate::style::ICON_BATTERY_FULL,
            time_to_empty: 0,
            time_to_full: 0,
            power_profiles: None,
        }
    }
}

fn battery_icon(percentage: f64, charging: bool) -> &'static str {
    use crate::style;
    if charging {
        if percentage >= 90.0 {
            style::ICON_BATTERY_CHARGING_FULL
        } else if percentage >= 70.0 {
            style::ICON_BATTERY_CHARGING_90
        } else if percentage >= 50.0 {
            style::ICON_BATTERY_CHARGING_60
        } else if percentage >= 30.0 {
            style::ICON_BATTERY_CHARGING_50
        } else if percentage >= 10.0 {
            style::ICON_BATTERY_CHARGING_30
        } else {
            style::ICON_BATTERY_CHARGING_20
        }
    } else if percentage >= 95.0 {
        style::ICON_BATTERY_FULL
    } else if percentage >= 80.0 {
        style::ICON_BATTERY_6
    } else if percentage >= 60.0 {
        style::ICON_BATTERY_5
    } else if percentage >= 45.0 {
        style::ICON_BATTERY_4
    } else if percentage >= 30.0 {
        style::ICON_BATTERY_3
    } else if percentage >= 15.0 {
        style::ICON_BATTERY_2
    } else if percentage >= 5.0 {
        style::ICON_BATTERY_1
    } else {
        style::ICON_BATTERY_0
    }
}

async fn read_power_profiles(conn: &zbus::Connection) -> Option<PowerProfileInfo> {
    let proxy: zbus::Proxy<'_> = zbus::proxy::Builder::new(conn)
        .destination("net.hadess.PowerProfiles")
        .ok()?
        .path("/net/hadess/PowerProfiles")
        .ok()?
        .interface("net.hadess.PowerProfiles")
        .ok()?
        .build()
        .await
        .ok()?;

    let active: String = proxy.get_property("ActiveProfile").await.ok()?;

    let profiles_raw: Vec<std::collections::HashMap<String, zbus::zvariant::OwnedValue>> =
        proxy.get_property("Profiles").await.ok()?;

    let available: Vec<String> = profiles_raw
        .iter()
        .filter_map(|dict| {
            dict.get("Profile")
                .and_then(|v| <String as TryFrom<_>>::try_from(v.clone()).ok())
        })
        .collect();

    if available.is_empty() {
        return None;
    }

    Some(PowerProfileInfo {
        available_profiles: available,
        active_profile: active,
    })
}

pub fn set_power_profile(profile: &str) {
    let profile = profile.to_string();
    tokio::spawn(async move {
        let Ok(conn) = zbus::Connection::system().await else {
            return;
        };
        let Some(proxy) = zbus::proxy::Builder::<zbus::Proxy<'_>>::new(&conn)
            .destination("net.hadess.PowerProfiles")
            .ok()
            .and_then(|b| b.path("/net/hadess/PowerProfiles").ok())
            .and_then(|b| b.interface("net.hadess.PowerProfiles").ok())
        else {
            return;
        };
        if let Ok(proxy) = proxy.build().await {
            if proxy.set_property("ActiveProfile", &profile).await.is_ok() {
                refresh_notify().notify_one();
            }
        }
    });
}

async fn build_upower_proxy(conn: &zbus::Connection) -> Option<zbus::Proxy<'_>> {
    zbus::proxy::Builder::new(conn)
        .destination("org.freedesktop.UPower")
        .ok()?
        .path("/org/freedesktop/UPower/devices/DisplayDevice")
        .ok()?
        .interface("org.freedesktop.UPower.Device")
        .ok()?
        .build()
        .await
        .ok()
}

async fn read_battery_dbus(proxy: &zbus::Proxy<'_>) -> Option<BatteryInfo> {
    let is_battery: bool = proxy.get_property("IsPresent").await.ok()?;
    if !is_battery {
        return None;
    }

    let percentage: f64 = proxy.get_property("Percentage").await.ok()?;
    let state: u32 = proxy.get_property("State").await.ok()?;
    let charging = matches!(state, 1 | 4 | 6);
    let time_to_empty: i64 = proxy.get_property("TimeToEmpty").await.unwrap_or(0);
    let time_to_full: i64 = proxy.get_property("TimeToFull").await.unwrap_or(0);

    Some(BatteryInfo {
        present: true,
        percentage,
        charging,
        icon_name: battery_icon(percentage, charging),
        time_to_empty,
        time_to_full,
        power_profiles: None,
    })
}

async fn read_full_state(
    upower_proxy: &zbus::Proxy<'_>,
    conn: &zbus::Connection,
) -> BatteryInfo {
    let mut info = read_battery_dbus(upower_proxy).await.unwrap_or_default();
    info.power_profiles = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        read_power_profiles(conn),
    )
    .await
    .unwrap_or(None);
    info
}

pub fn stream() -> impl Stream<Item = BatteryInfo> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        loop {
            let conn = loop {
                if let Ok(c) = zbus::Connection::system().await {
                    break c;
                }
                log::warn!("battery: failed to connect to system D-Bus, retrying");
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            };

            if run_battery_loop(&conn, &tx).await.is_err() {
                log::warn!("battery: signal loop ended, reconnecting");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    });

    tokio_stream::wrappers::UnboundedReceiverStream::new(rx)
}

async fn run_battery_loop(
    conn: &zbus::Connection,
    tx: &tokio::sync::mpsc::UnboundedSender<BatteryInfo>,
) -> Result<(), ()> {
    let upower_proxy = build_upower_proxy(conn).await.ok_or(())?;

    // Subscribe to PropertiesChanged on UPower device
    let upower_props_proxy = zbus::fdo::PropertiesProxy::builder(conn)
        .destination("org.freedesktop.UPower")
        .map_err(|_| ())?
        .path("/org/freedesktop/UPower/devices/DisplayDevice")
        .map_err(|_| ())?
        .build()
        .await
        .map_err(|_| ())?;
    let mut upower_signals = upower_props_proxy
        .receive_properties_changed()
        .await
        .map_err(|_| ())?;

    // Subscribe to PropertiesChanged on PowerProfiles (optional)
    let mut pp_signals = async {
        let proxy = zbus::fdo::PropertiesProxy::builder(conn)
            .destination("net.hadess.PowerProfiles")
            .ok()?
            .path("/net/hadess/PowerProfiles")
            .ok()?
            .build()
            .await
            .ok()?;
        proxy.receive_properties_changed().await.ok()
    }
    .await;

    // Emit initial state
    let info = read_full_state(&upower_proxy, conn).await;
    tx.send(info).map_err(|_| ())?;

    loop {
        tokio::select! {
            Some(_) = upower_signals.next() => {}
            Some(_) = async {
                match pp_signals.as_mut() {
                    Some(s) => s.next().await,
                    None => std::future::pending().await,
                }
            } => {}
            () = refresh_notify().notified() => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            // Fallback refresh every 5 minutes in case signals are missed
            () = tokio::time::sleep(std::time::Duration::from_mins(5)) => {}
        }

        let info = read_full_state(&upower_proxy, conn).await;
        tx.send(info).map_err(|_| ())?;
    }
}
