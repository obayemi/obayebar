use futures_util::Stream;

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

async fn read_battery_dbus() -> Option<BatteryInfo> {
    let conn = zbus::Connection::system().await.ok()?;

    let proxy: zbus::Proxy<'_> = zbus::proxy::Builder::new(&conn)
        .destination("org.freedesktop.UPower")
        .ok()?
        .path("/org/freedesktop/UPower/devices/DisplayDevice")
        .ok()?
        .interface("org.freedesktop.UPower.Device")
        .ok()?
        .build()
        .await
        .ok()?;

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
    })
}

pub fn stream() -> impl Stream<Item = BatteryInfo> {
    futures_util::stream::unfold((), |()| async {
        let info = read_battery_dbus().await.unwrap_or_default();
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        Some((info, ()))
    })
}
