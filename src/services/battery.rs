use std::sync::OnceLock;

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

    // Profiles is aa{sv} — array of dicts, each with a "Profile" string key
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
        let Some(proxy) = (zbus::proxy::Builder::<zbus::Proxy<'_>>::new(&conn)
            .destination("net.hadess.PowerProfiles")
            .ok()
            .and_then(|b| b.path("/net/hadess/PowerProfiles").ok())
            .and_then(|b| b.interface("net.hadess.PowerProfiles").ok()))
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

pub fn stream() -> impl Stream<Item = BatteryInfo> {
    futures_util::stream::unfold(
        (None, false),
        |(conn, should_sleep): (Option<zbus::Connection>, bool)| async move {
            if should_sleep {
                tokio::select! {
                    () = tokio::time::sleep(std::time::Duration::from_secs(30)) => {}
                    () = refresh_notify().notified() => {
                        // Small delay to let D-Bus property propagate
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
            let connection = if let Some(c) = conn {
                c
            } else if let Ok(c) = zbus::Connection::system().await {
                c
            } else {
                log::warn!("battery: failed to connect to system D-Bus");
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                return Some((BatteryInfo::default(), (None, true)));
            };
            let mut info = if let Some(proxy) = build_upower_proxy(&connection).await {
                read_battery_dbus(&proxy).await.unwrap_or_default()
            } else {
                log::debug!("battery: UPower proxy unavailable");
                BatteryInfo::default()
            };
            info.power_profiles = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                read_power_profiles(&connection),
            )
            .await
            .unwrap_or(None);
            Some((info, (Some(connection), true)))
        },
    )
}
