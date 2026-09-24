//! Whether the compositor already considers the session locked.
//!
//! `hyprland_lock_notifier_v1` is Hyprland's own bookkeeping: binding it and
//! calling `get_lock_notification` makes the compositor send `locked`
//! immediately if a lock is up. The protocol guarantees it: "If the session
//! is already locked when calling this method, the locked event shall be
//! sent immediately." `locked` itself is only sent once the lock client has
//! presented a frame on every output, so a session mid-transition still
//! comes back not-locked until that frame is up.

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle};
use wayland_protocols_hyprland::lock_notify::v1::client::hyprland_lock_notification_v1::{
    self, HyprlandLockNotificationV1,
};
use wayland_protocols_hyprland::lock_notify::v1::client::hyprland_lock_notifier_v1::HyprlandLockNotifierV1;

/// Ask the compositor whether the session is locked right now.
///
/// Blocking, and bounded to the couple of roundtrips the protocol needs:
/// never a wait for something that may never come. `true` is the only
/// definite answer: a session mid-transition, a missing
/// `hyprland_lock_notifier_v1` global, no Wayland display, or the exchange
/// failing some other way all come back `false`, which means "not known to
/// be locked", never proof that the session is unlocked.
#[must_use]
pub fn session_locked() -> bool {
    try_query().unwrap_or(false)
}

#[derive(Default)]
struct Reply {
    locked: bool,
}

const fn is_locked(event: &hyprland_lock_notification_v1::Event) -> bool {
    matches!(event, hyprland_lock_notification_v1::Event::Locked)
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Reply {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(Reply: ignore HyprlandLockNotifierV1);

impl Dispatch<HyprlandLockNotificationV1, ()> for Reply {
    fn event(
        state: &mut Self,
        _: &HyprlandLockNotificationV1,
        event: hyprland_lock_notification_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.locked = is_locked(&event);
    }
}

/// The actual exchange, `?`-chained through whichever step fails first.
fn try_query() -> Option<bool> {
    let conn = Connection::connect_to_env().ok()?;
    let (globals, mut event_queue) = registry_queue_init::<Reply>(&conn).ok()?;
    let qh = event_queue.handle();

    let notifier: HyprlandLockNotifierV1 = globals.bind(&qh, 1..=1, ()).ok()?;
    let mut state = Reply::default();
    notifier.get_lock_notification(&qh, ());

    event_queue.roundtrip(&mut state).ok()?;

    Some(state.locked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wayland_protocols_hyprland::lock_notify::v1::client::hyprland_lock_notification_v1::Event;

    #[test]
    fn locked_event_is_locked() {
        assert!(is_locked(&Event::Locked));
    }

    #[test]
    fn unlocked_event_is_not_locked() {
        assert!(!is_locked(&Event::Unlocked));
    }

    #[test]
    fn reply_starts_not_locked() {
        assert!(!Reply::default().locked);
    }
}
