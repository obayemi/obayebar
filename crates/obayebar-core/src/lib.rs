//! Helpers shared by every obayebar binary.
//!
//! The one rule this crate enforces is that it does **not** depend on iced. The
//! bar needs a GUI stack; the lock screen is a one-shot that renders a text
//! config and execs hyprlock, and the wallpaper renderer talks wlr-layer-shell
//! directly. Keeping the shared code iced-free is what lets those two build in
//! seconds instead of compiling several hundred crates.

pub mod wallpaper;
pub mod xdg;
