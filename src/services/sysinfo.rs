use futures_util::Stream;

#[derive(Debug, Clone)]
pub struct SysInfo {
    pub cpu_percent: f32,
    pub ram_percent: f32,
}

impl Default for SysInfo {
    fn default() -> Self {
        Self {
            cpu_percent: 0.0,
            ram_percent: 0.0,
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

pub fn stream() -> impl Stream<Item = SysInfo> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        let mut prev_idle: u64 = 0;
        let mut prev_total: u64 = 0;

        loop {
            let cpu_percent = tokio::fs::read_to_string("/proc/stat")
                .await
                .ok()
                .and_then(|stat| parse_cpu_stat(&stat))
                .map_or(0.0, |(idle, total)| {
                    let pct = compute_cpu_percent(prev_idle, prev_total, idle, total);
                    prev_idle = idle;
                    prev_total = total;
                    pct
                });

            let ram_percent = tokio::fs::read_to_string("/proc/meminfo")
                .await
                .ok()
                .and_then(|content| parse_meminfo(&content))
                .unwrap_or(0.0);

            let info = SysInfo {
                cpu_percent,
                ram_percent,
            };

            if tx.send(info).is_err() {
                break;
            }

            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });

    tokio_stream::wrappers::UnboundedReceiverStream::new(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cpu_stat_typical() {
        let content = "cpu  1234 56 789 5000 100 10 5 3 0 0\ncpu0  600 28 400 2500 50 5 2 1 0 0\n";
        let (idle, total) = parse_cpu_stat(content).unwrap();
        // idle = 5000 + 100 = 5100
        // total = 1234 + 56 + 789 + 5000 + 100 + 10 + 5 + 3 = 7197
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
        // prev: idle=100, total=200 -> next: idle=150, total=400
        // delta_idle=50, delta_total=200 -> usage = 1 - 50/200 = 0.75 = 75%
        let pct = compute_cpu_percent(100, 200, 150, 400);
        assert!((pct - 75.0).abs() < 0.1);
    }

    #[test]
    fn cpu_percent_zero_delta() {
        let pct = compute_cpu_percent(100, 200, 100, 200);
        assert!((pct - 0.0).abs() < 0.01);
    }
}
