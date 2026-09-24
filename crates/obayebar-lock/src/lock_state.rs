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
    Event, HyprlandLockNotificationV1,
};
use wayland_protocols_hyprland::lock_notify::v1::client::hyprland_lock_notifier_v1::HyprlandLockNotifierV1;

/// Ask the compositor whether the session is locked right now.
///
/// Blocks on a Wayland roundtrip with no timeout of its own: a compositor
/// that accepts the connection but never answers leaves this call waiting.
/// `true` is the only definite answer: a session mid-transition, a missing
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
        event: Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.locked = matches!(event, Event::Locked);
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
