//! Generated client bindings for `hyprland_lock_notifier_v1`.
//!
//! Produced by `wayland-scanner` from `protocols/hyprland-lock-notify-v1.xml`
//! (vendored from `hyprland-protocols`), not written by hand: the usual
//! clippy bar does not apply to this module.
#![allow(
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::cargo,
    missing_debug_implementations
)]

use wayland_client;

pub mod __interfaces {
    wayland_scanner::generate_interfaces!("protocols/hyprland-lock-notify-v1.xml");
}
use self::__interfaces::*;

wayland_scanner::generate_client_code!("protocols/hyprland-lock-notify-v1.xml");
