//! Event-driven timers that replace the 1 Hz Tick subscription.
//!
//! The bar no longer needs to wake every second: the clock only changes on
//! minute boundaries, and popup expiry is driven by absolute timestamps.

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
        let delay = (at - Local::now())
            .to_std()
            .unwrap_or(std::time::Duration::ZERO);
        tokio::time::sleep(delay).await;
        let _ = tx.send(());
    });

    UnboundedReceiverStream::new(rx)
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
