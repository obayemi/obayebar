//! Event-driven timers: wake only when something actually changes.
//!
//! The clock only changes on minute boundaries, and popup expiry is driven
//! by absolute timestamps, so neither needs a 1 Hz wake-up.

use chrono::{DateTime, Local, Timelike};
use futures_util::Stream;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Wake once at the next wall-clock minute boundary, forever.
pub fn clock_stream() -> impl Stream<Item = DateTime<Local>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        loop {
            let now = Local::now();
            let delay = duration_until_next_minute(&now);
            tokio::time::sleep(delay).await;
            if tx.send(Local::now()).is_err() {
                break;
            }
        }
    });

    UnboundedReceiverStream::new(rx)
}

/// Wake once at the given wall-clock instant. Used by the subscription
/// machinery to fire when the earliest popup expires.
pub fn wake_at(at: DateTime<Local>) -> impl Stream<Item = ()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        let delay = duration_until(at, Local::now());
        tokio::time::sleep(delay).await;
        let _ = tx.send(());
    });

    UnboundedReceiverStream::new(rx)
}

/// How long to sleep to reach `at` from `now`. A past or present `at`
/// sleeps for zero, rather than letting the signed subtraction underflow
/// `std::time::Duration`.
fn duration_until(at: DateTime<Local>, now: DateTime<Local>) -> std::time::Duration {
    at.signed_duration_since(now)
        .to_std()
        .unwrap_or(std::time::Duration::ZERO)
}

fn duration_until_next_minute(now: &DateTime<Local>) -> std::time::Duration {
    // Nanoseconds until the next `:00` second. Clamp to at least 1ms so a
    // rounding edge can't leave us spin-waking in a tight loop.
    let ns_into_minute = u64::from(now.second())
        .saturating_mul(1_000_000_000)
        .saturating_add(u64::from(now.nanosecond()));
    let remaining = 60_u64
        .saturating_mul(1_000_000_000)
        .saturating_sub(ns_into_minute);
    std::time::Duration::from_nanos(remaining.max(1_000_000))
}

#[cfg(test)]
mod tests {
    use super::duration_until;
    use chrono::Duration;

    fn fixed_instant() -> super::DateTime<super::Local> {
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH.with_timezone(&super::Local)
    }

    #[test]
    fn future_instant_sleeps_for_the_gap() {
        let now = fixed_instant();
        let at = now + Duration::seconds(5);
        let delay = duration_until(at, now);
        assert_eq!(delay, std::time::Duration::from_secs(5));
    }

    #[test]
    fn past_instant_sleeps_for_zero() {
        let now = fixed_instant();
        let at = now - Duration::seconds(5);
        assert_eq!(duration_until(at, now), std::time::Duration::ZERO);
    }

    #[test]
    fn same_instant_sleeps_for_zero() {
        let now = fixed_instant();
        assert_eq!(duration_until(now, now), std::time::Duration::ZERO);
    }
}
