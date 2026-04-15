use futures_util::Stream;

#[derive(Debug, Clone, PartialEq)]
pub struct SysInfo {
    pub cpu_percent: f32,
    pub cpu_temp_c: Option<f32>,
    pub gpu_percent: f32,
    pub gpu_temp_c: Option<f32>,
    pub ram_percent: f32,
    /// Network download bytes/s
    pub net_rx_rate: u64,
    /// Network upload bytes/s
    pub net_tx_rate: u64,
}

impl Default for SysInfo {
    fn default() -> Self {
        Self {
            cpu_percent: 0.0,
            cpu_temp_c: None,
            gpu_percent: 0.0,
            gpu_temp_c: None,
            ram_percent: 0.0,
            net_rx_rate: 0,
            net_tx_rate: 0,
        }
    }
}

/// Parse the first `cpu` line from `/proc/stat` and return `(idle, total)`.
fn parse_cpu_stat(content: &str) -> Option<(u64, u64)> {
    let line = content.lines().find(|l| l.starts_with("cpu "))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1) // skip "cpu"
        .take(8) // user nice system idle iowait irq softirq steal
        .filter_map(|f| f.parse().ok())
        .collect();
    if fields.len() < 4 {
        return None;
    }
    let idle = fields
        .get(3)
        .copied()
        .unwrap_or(0)
        .saturating_add(fields.get(4).copied().unwrap_or(0));
    let total: u64 = fields.iter().copied().fold(0u64, u64::saturating_add);
    Some((idle, total))
}

/// Parse `/proc/meminfo` and return used percentage.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn parse_meminfo(content: &str) -> Option<f32> {
    let mut mem_total: Option<u64> = None;
    let mut mem_available: Option<u64> = None;

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            mem_total = rest
                .trim()
                .strip_suffix("kB")
                .and_then(|v| v.trim().parse().ok());
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            mem_available = rest
                .trim()
                .strip_suffix("kB")
                .and_then(|v| v.trim().parse().ok());
        }
        if mem_total.is_some() && mem_available.is_some() {
            break;
        }
    }

    let total = mem_total? as f64;
    let available = mem_available? as f64;
    if total <= 0.0 {
        return None;
    }

    Some(((total - available) / total * 100.0) as f32)
}

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn compute_cpu_percent(prev_idle: u64, prev_total: u64, idle: u64, total: u64) -> f32 {
    let idle_delta = idle.saturating_sub(prev_idle);
    let total_delta = total.saturating_sub(prev_total);
    if total_delta > 0 {
        ((1.0 - (idle_delta as f64 / total_delta as f64)) * 100.0) as f32
    } else {
        0.0
    }
}

/// GPU backend detection result.
enum GpuBackend {
    /// AMD/Intel via sysfs `gpu_busy_percent` + optional hwmon temp path
    Sysfs {
        busy_path: String,
        temp_path: Option<String>,
    },
    /// NVIDIA via NVML library
    Nvml(Box<nvml_wrapper::Nvml>),
}

impl std::fmt::Debug for GpuBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sysfs {
                busy_path,
                temp_path,
            } => f
                .debug_struct("Sysfs")
                .field("busy_path", busy_path)
                .field("temp_path", temp_path)
                .finish(),
            Self::Nvml(_) => f.debug_tuple("Nvml").finish(),
        }
    }
}

/// Detect available GPU monitoring backend.
async fn detect_gpu_backend() -> Option<GpuBackend> {
    // Try AMD/Intel sysfs first
    if let Ok(mut dir) = tokio::fs::read_dir("/sys/class/drm").await {
        while let Ok(Some(entry)) = dir.next_entry().await {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("card") && !name.contains('-') {
                let busy_path = format!("/sys/class/drm/{name}/device/gpu_busy_percent");
                if tokio::fs::metadata(&busy_path).await.is_ok() {
                    let temp_path =
                        find_hwmon_temp(&format!("/sys/class/drm/{name}/device/hwmon")).await;
                    return Some(GpuBackend::Sysfs {
                        busy_path,
                        temp_path,
                    });
                }
            }
        }
    }

    // Try NVIDIA via NVML library
    if let Ok(nvml) = nvml_wrapper::Nvml::init() {
        if nvml.device_count().unwrap_or(0) > 0 {
            return Some(GpuBackend::Nvml(Box::new(nvml)));
        }
    }

    None
}

/// Find the first `temp1_input` under an hwmon directory.
async fn find_hwmon_temp(hwmon_dir: &str) -> Option<String> {
    let mut dir = tokio::fs::read_dir(hwmon_dir).await.ok()?;
    while let Ok(Some(entry)) = dir.next_entry().await {
        let path = format!("{}/temp1_input", entry.path().display());
        if tokio::fs::metadata(&path).await.is_ok() {
            return Some(path);
        }
    }
    None
}

/// Read GPU usage and temperature from the detected backend.
async fn read_gpu_info(backend: &GpuBackend) -> (f32, Option<f32>) {
    match backend {
        GpuBackend::Sysfs {
            busy_path,
            temp_path,
        } => {
            let percent = read_sysfs_f32(busy_path).await.unwrap_or(0.0);
            let temp = match temp_path {
                Some(p) => read_sysfs_f32(p).await.map(|v| v / 1000.0), // millidegrees → °C
                None => None,
            };
            (percent, temp)
        }
        GpuBackend::Nvml(nvml) => {
            let Ok(device) = nvml.device_by_index(0) else {
                return (0.0, None);
            };
            #[allow(clippy::cast_precision_loss)]
            let percent = device.utilization_rates().map_or(0.0, |u| u.gpu as f32);
            #[allow(clippy::cast_precision_loss)]
            let temp = device
                .temperature(nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu)
                .ok()
                .map(|t| t as f32);
            (percent, temp)
        }
    }
}

async fn read_sysfs_f32(path: &str) -> Option<f32> {
    tokio::fs::read_to_string(path)
        .await
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Find the CPU/SoC thermal zone temperature path.
async fn find_cpu_temp_path() -> Option<String> {
    // Try thermal_zone type "x86_pkg_temp" or similar
    if let Ok(mut dir) = tokio::fs::read_dir("/sys/class/thermal").await {
        while let Ok(Some(entry)) = dir.next_entry().await {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("thermal_zone") {
                continue;
            }
            let type_path = format!("/sys/class/thermal/{name}/type");
            if let Ok(t) = tokio::fs::read_to_string(&type_path).await {
                let t = t.trim();
                // Common CPU thermal zone type names
                if t.contains("x86_pkg")
                    || t.contains("cpu")
                    || t == "k10temp"
                    || t == "coretemp"
                    || t == "acpitz"
                {
                    let temp_path = format!("/sys/class/thermal/{name}/temp");
                    if tokio::fs::metadata(&temp_path).await.is_ok() {
                        return Some(temp_path);
                    }
                }
            }
        }
    }
    // Fallback: first thermal_zone with a temp file
    if let Ok(mut dir) = tokio::fs::read_dir("/sys/class/thermal").await {
        while let Ok(Some(entry)) = dir.next_entry().await {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("thermal_zone") {
                let temp_path = format!("/sys/class/thermal/{name}/temp");
                if tokio::fs::metadata(&temp_path).await.is_ok() {
                    return Some(temp_path);
                }
            }
        }
    }
    None
}

/// Read CPU temperature in °C from sysfs (millidegrees).
async fn read_cpu_temp(path: &str) -> Option<f32> {
    read_sysfs_f32(path).await.map(|v| v / 1000.0)
}

/// Parse `/proc/net/dev` and return `(total_rx_bytes, total_tx_bytes)` across
/// all non-loopback interfaces.
fn parse_net_dev(content: &str) -> (u64, u64) {
    let mut rx_total: u64 = 0;
    let mut tx_total: u64 = 0;

    for line in content.lines().skip(2) {
        // Format: "  iface: rx_bytes rx_packets ... tx_bytes tx_packets ..."
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        let iface = iface.trim();
        if iface == "lo" {
            continue;
        }
        let fields: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|f| f.parse().ok())
            .collect();
        // rx_bytes is field 0, tx_bytes is field 8
        if let Some(&rx) = fields.first() {
            rx_total = rx_total.saturating_add(rx);
        }
        if let Some(&tx) = fields.get(8) {
            tx_total = tx_total.saturating_add(tx);
        }
    }
    (rx_total, tx_total)
}

pub fn stream() -> impl Stream<Item = SysInfo> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        let gpu_backend = detect_gpu_backend().await;
        match &gpu_backend {
            Some(backend) => log::info!("sysinfo: GPU backend: {backend:?}"),
            None => log::info!("sysinfo: no GPU backend found, GPU usage will be 0"),
        }

        let cpu_temp_path = find_cpu_temp_path().await;
        if let Some(ref p) = cpu_temp_path {
            log::info!("sysinfo: CPU temp path: {p}");
        }

        let mut prev_cpu_idle: u64 = 0;
        let mut prev_cpu_total: u64 = 0;
        let mut prev_net_rx: u64 = 0;
        let mut prev_net_tx: u64 = 0;
        let mut last = SysInfo::default();

        loop {
            // CPU
            let cpu_percent = tokio::fs::read_to_string("/proc/stat")
                .await
                .ok()
                .and_then(|stat| parse_cpu_stat(&stat))
                .map_or(0.0, |(idle, total)| {
                    let pct = compute_cpu_percent(prev_cpu_idle, prev_cpu_total, idle, total);
                    prev_cpu_idle = idle;
                    prev_cpu_total = total;
                    pct
                });

            // CPU temp
            let cpu_temp_c = match &cpu_temp_path {
                Some(p) => read_cpu_temp(p).await,
                None => None,
            };

            // GPU
            let (gpu_percent, gpu_temp_c) = match &gpu_backend {
                Some(backend) => read_gpu_info(backend).await,
                None => (0.0, None),
            };

            // RAM
            let ram_percent = tokio::fs::read_to_string("/proc/meminfo")
                .await
                .ok()
                .and_then(|content| parse_meminfo(&content))
                .unwrap_or(0.0);

            // Network
            let (net_rx, net_tx) = tokio::fs::read_to_string("/proc/net/dev")
                .await
                .ok()
                .map_or((0, 0), |content| parse_net_dev(&content));

            let rx_rate = net_rx.saturating_sub(prev_net_rx) / 2; // 2s interval
            let tx_rate = net_tx.saturating_sub(prev_net_tx) / 2;
            prev_net_rx = net_rx;
            prev_net_tx = net_tx;

            let info = SysInfo {
                cpu_percent,
                cpu_temp_c,
                gpu_percent,
                gpu_temp_c,
                ram_percent,
                net_rx_rate: rx_rate,
                net_tx_rate: tx_rate,
            };

            if info != last {
                last = info.clone();
                if tx.send(info).is_err() {
                    break;
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });

    tokio_stream::wrappers::UnboundedReceiverStream::new(rx)
}

/// Format bytes/s as a human-readable rate string.
pub fn format_rate(bytes_per_sec: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;

    if bytes_per_sec >= GB {
        #[allow(clippy::cast_precision_loss)]
        let val = bytes_per_sec as f64 / GB as f64;
        format!("{val:.1} G")
    } else if bytes_per_sec >= MB {
        #[allow(clippy::cast_precision_loss)]
        let val = bytes_per_sec as f64 / MB as f64;
        format!("{val:.1} M")
    } else if bytes_per_sec >= KB {
        #[allow(clippy::cast_precision_loss)]
        let val = bytes_per_sec as f64 / KB as f64;
        format!("{val:.0} K")
    } else {
        format!("{bytes_per_sec} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cpu_stat_typical() {
        let content = "cpu  1234 56 789 5000 100 10 5 3 0 0\ncpu0  600 28 400 2500 50 5 2 1 0 0\n";
        let (idle, total) = parse_cpu_stat(content).unwrap();
        assert_eq!(idle, 5100);
        assert_eq!(total, 7197);
    }

    #[test]
    fn parse_cpu_stat_missing_returns_none() {
        assert!(parse_cpu_stat("").is_none());
        assert!(parse_cpu_stat("cpuinfo blah").is_none());
    }

    #[test]
    fn parse_meminfo_typical() {
        let content =
            "MemTotal:       16384000 kB\nMemFree:         1000000 kB\nMemAvailable:    8192000 kB\n";
        let pct = parse_meminfo(content).unwrap();
        assert!((pct - 50.0).abs() < 0.1);
    }

    #[test]
    fn parse_meminfo_missing_returns_none() {
        assert!(parse_meminfo("").is_none());
        assert!(parse_meminfo("MemTotal: 1000 kB\n").is_none());
    }

    #[test]
    fn cpu_percent_calculation() {
        let pct = compute_cpu_percent(100, 200, 150, 400);
        assert!((pct - 75.0).abs() < 0.1);
    }

    #[test]
    fn cpu_percent_zero_delta() {
        let pct = compute_cpu_percent(100, 200, 100, 200);
        assert!((pct - 0.0).abs() < 0.01);
    }

    #[test]
    fn parse_net_dev_typical() {
        let content = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1000    10    0    0    0     0          0         0     2000    20    0    0    0     0       0          0
  eth0: 50000   100   0    0    0     0          0         0     30000   80    0    0    0     0       0          0
 wlan0: 20000   50    0    0    0     0          0         0     10000   30    0    0    0     0       0          0
";
        let (rx, tx) = parse_net_dev(content);
        // lo excluded, eth0 + wlan0
        assert_eq!(rx, 70000);
        assert_eq!(tx, 40000);
    }

    #[test]
    fn format_rate_scales() {
        assert_eq!(format_rate(500), "500 B");
        assert_eq!(format_rate(2048), "2 K");
        assert_eq!(format_rate(1_500_000), "1.4 M");
        assert_eq!(format_rate(2_500_000_000), "2.3 G");
    }
}
