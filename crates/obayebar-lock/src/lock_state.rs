//! Whether the compositor already considers the session locked.
//!
//! `hyprland_lock_notifier_v1` is Hyprland's own bookkeeping: binding it and
//! calling `get_lock_notification` makes the compositor send `locked`
//! immediately if a lock is up (hypridle relies on the same guarantee to know
//! when to stop polling). Asking the compositor instead of trusting a
//! systemd unit's state is what lets a takeover tell a live lock screen apart
//! from a hung one — both hold the same unit up.

mod protocol;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};

use protocol::hyprland_lock_notify_v1::hyprland_lock_notification_v1::{
    self, HyprlandLockNotificationV1,
};
use protocol::hyprland_lock_notify_v1::hyprland_lock_notifier_v1::{self, HyprlandLockNotifierV1};

/// What the compositor says about the session lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    /// The compositor sent `locked` for the current session.
    Locked,
    /// The compositor answered, and it was not `locked`.
    Unlocked,
    /// No Wayland display, no `hyprland_lock_notifier_v1` global, or the
    /// exchange failed some other way. Not a compositor that said "no".
    Unknown,
}

/// Ask the compositor whether the session is locked right now.
///
/// Blocking, and bounded to the couple of roundtrips the protocol needs:
/// never a wait for something that may never come. Anything short of a
/// definite answer comes back as [`LockState::Unknown`], which a caller must
/// treat like "don't know", not like "unlocked".
#[must_use]
pub fn query() -> LockState {
    try_query().unwrap_or(LockState::Unknown)
}

#[derive(Default)]
struct Notification {
    locked: Option<bool>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Notification {
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

impl Dispatch<HyprlandLockNotifierV1, ()> for Notification {
    fn event(
        _: &mut Self,
        _: &HyprlandLockNotifierV1,
        event: hyprland_lock_notifier_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {}
    }
}

impl Dispatch<HyprlandLockNotificationV1, ()> for Notification {
    fn event(
        state: &mut Self,
        _: &HyprlandLockNotificationV1,
        event: hyprland_lock_notification_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.locked = Some(matches!(
            event,
            hyprland_lock_notification_v1::Event::Locked
        ));
    }
}

/// The actual exchange, `?`-chained through whichever step fails first.
fn try_query() -> Option<LockState> {
    let conn = Connection::connect_to_env().ok()?;
    let (globals, mut event_queue) = registry_queue_init::<Notification>(&conn).ok()?;
    let qh = event_queue.handle();

    let notifier: HyprlandLockNotifierV1 = globals.bind(&qh, 1..=1, ()).ok()?;
    let mut state = Notification::default();
    notifier.get_lock_notification(&qh, ());

    event_queue.roundtrip(&mut state).ok()?;

    Some(if state.locked.unwrap_or(false) {
        LockState::Locked
    } else {
        LockState::Unlocked
    })
}
