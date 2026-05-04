# obayebar

> Bar-shell thing, **HEAVILY** inspired by [caelestia-shell](https://github.com/caelestia-dots/caelestia),
> but with less fluff, made for performance and for specifically me.

A vertical Wayland status bar for Hyprland, written in Rust. obayebar exists
because I wanted the caelestia look and feel without the QtQuick stack and
without the resource footprint of a full shell. It is built specifically for
my usage on Hyprland — only the modules I feel like I need, no plugin system,
no general-purpose abstraction layer. Hardware-wise it isn't picky (anything
with wgpu and PipeWire works), but anything outside *"things I personally
want on my bar"* is out of scope on purpose.

## What it is

A single, statically-typed binary (~27 MB unstripped) that draws a left-anchored
vertical bar on every Hyprland output, plus a few satellite layer-shell windows
for popups and panels:

- one bar per connected monitor, automatically respawned when outputs come back
  from sleep / disconnect
- a dbus notification daemon with a stacked popup overlay
- click-to-open settings panels: audio, network, bluetooth, battery / power
  profile, sysinfo, optional GitLab todos
- a separate launcher binary (`obayebar-launcher`) that draws an app-launcher
  layer surface

## Modules on the bar

| Module          | Source                                   | Notes                                                                    |
|-----------------|------------------------------------------|--------------------------------------------------------------------------|
| Workspaces      | Hyprland IPC (`j/workspaces`, socket2)   | Per-monitor, animated indicator with a small physics spring              |
| Active window   | Hyprland IPC (`activewindow` event)      | Class + title, vertical text rendered to a canvas                        |
| System tray     | StatusNotifierItem (dbus)                | Click → activate, with cached icons                                      |
| GitLab todos    | GitLab REST API + Secret Service keyring | Opt-in via `--gitlab` / config / home-manager option                     |
| Clock           | local time tick                          |                                                                          |
| Audio           | PipeWire (native, via `pipewire-rs`)     | Volume, mute, sink switching, panel with sliders                         |
| Network         | NetworkManager (dbus)                    | Wi-Fi list, connect/disconnect, wired indicator                          |
| Bluetooth       | BlueZ (dbus)                             | Adapter on/off, discovery, paired devices, forget                        |
| Battery / power | UPower + `power-profiles-daemon` (dbus)  | Percentage, profile switching                                            |
| Sysinfo         | `/proc`, NVML                            | CPU + GPU + RAM usage, network rates, threshold colouring                |
| Notifications   | `org.freedesktop.Notifications` (dbus)   | Replaces `mako` / `dunst`, stacks with overflow summary at 2/5 of screen |

Configuration lives in `$XDG_CONFIG_HOME/obayebar/config.toml` (currently the
GitLab module is the only thing exposed there); CLI flags override the file,
env vars override that. A home-manager module ships in `flake.nix`.

## Why it stays light

The whole point of this rewrite was to **not** be caelestia-shell. Concrete
choices that follow from that:

- **No QtQuick, no JavaScript, no shell runtime.** Just a Rust binary on
  [`iced`](https://iced.rs) + [`iced_layershell`](https://github.com/waycrate/exwlshelleventloop)
  driving wlr-layer-shell directly. No QML interpreter, no V8, no Qt scene graph.
- **wgpu renderer with aggressive lazy/cached widgets.** Workspace indicators
  are drawn on a `canvas::Cache` that is only invalidated when state actually
  changes; clock / status / tray sections are wrapped in `iced::widget::lazy`
  with hand-rolled cache keys so a CPU/RAM number bumping by 0.1 % doesn't
  rebuild the widget tree. Spring animation only ticks at 60 Hz **while it is
  animating** — the bar is fully idle (no wake-ups, no draws) when nothing on
  screen is moving.
- **Push-only event sources, never polling.**
  - Hyprland: one persistent `socket2` connection, parsed line by line, and
    only events that actually affect the rendered state
    (workspace/window/monitor changes) cause a refresh — high-frequency noise
    like `activewindowv2` and `windowtitle` is dropped without waking the UI
    thread.
  - dbus services (network, bluetooth, notifications, battery, power-profiles,
    upower, gitlab, tray) all use signal subscriptions via `zbus`.
  - Audio comes straight from PipeWire's native protocol (`pipewire-rs`), not
    from `pactl` or polling `pavucontrol`.
- **Per-second clock, not per-frame.** The clock uses a custom timer
  subscription (`services::timers::clock_stream`) that wakes exactly on the
  next minute boundary and on the next pending notification expiry — never on
  a fixed interval.
- **Notification popup is auto-sized.** The popup window resizes to fit the
  current notifications and capped at 2/5 of the focused monitor's logical
  height; anything that doesn't fit is collapsed into a single
  "*N more notifications*" entry rather than rendering offscreen widgets.
- **Launcher cache.** `obayebar-launcher` persists desktop-entry parsing and
  resolved icon paths to `XDG_CACHE_HOME` and launch frequencies to
  `XDG_DATA_HOME`, so cold start is almost instant after the first run.
- **Smithay clipboard worker disabled.** No surface in the bar is
  keyboard-interactive, so the upstream always-on clipboard thread is
  switched off via `iced_layershell::disable_clipboard()`. (The launcher,
  which *is* interactive, runs in its own process.)
- **Secrets stored in the kernel keyring.** GitLab tokens go through Secret
  Service when available, falling back to a file in `XDG_CONFIG_HOME`. The
  token never ends up in the Nix store even via the home-manager module.
- **Defensive multi-monitor handling.** When the compositor tears down a
  layer surface (output disappears across screen sleep, monitor disconnect,
  etc.), `iced::window::close_events()` is observed and the bar is
  re-spawned pinned to the original output, so all monitors get their bar
  back on wake instead of piling onto one screen.

The last bullet describes a behaviour you specifically should not have to
think about — it just works.

## Libraries

| Crate                        | Used for                                                  |
|------------------------------|-----------------------------------------------------------|
| `iced` 0.14                  | Reactive UI runtime, wgpu renderer, canvas, lazy widgets  |
| `iced_layershell` 0.18-beta4 | wlr-layer-shell integration on top of iced                |
| `zbus` 5                     | Async dbus for NetworkManager / BlueZ / UPower / SNI / …  |
| `pipewire` 0.9               | Native PipeWire client for audio                          |
| `tokio` 1.x                  | Async runtime, signal/timer plumbing                      |
| `chrono`                     | Time + minute-aligned wakeups                             |
| `nvml-wrapper`               | NVIDIA GPU usage / temperature                            |
| `fuzzy-matcher` (Skim)       | Launcher fuzzy ranking                                    |
| `resvg` + `image`            | Tray / launcher icon decoding                             |
| `reqwest` (rustls)           | GitLab REST API                                           |
| `secret-service`             | Storing the GitLab PAT in the kernel keyring              |
| `serde` + `toml`             | Config file parsing                                       |
| `ab_glyph` + `fontdb`        | Vector text rendering on the workspace canvas             |

## Build & run

The project pulls in a few things the toolchain on most distros won't have
matched up out of the box:

- **Nightly Rust** — needed for `cargo-features = ["codegen-backend"]` and
  the `rustc-codegen-cranelift-preview` component used as the dev codegen
  backend. The Cranelift backend is what makes incremental dev builds fast;
  release builds still go through LLVM.
- **`mold`** — used as the linker. Iced + wgpu + `pipewire-rs` pull in a
  lot of object files; `mold` cuts link time roughly in half versus `lld`
  and a lot more versus the default GNU `ld`. Configured via `.cargo`.
- **System libs** — `wayland`, `libxkbcommon`, `vulkan-loader`, `fontconfig`,
  `pipewire`, plus `pkg-config` / `clang` / `libclang` at build time.
- **Material Symbols font** — looked up at runtime via `OBAYEBAR_FONT_DIR`.

Because of all that, **the recommended way to build or hack on the project
is the Nix dev shell**:

```sh
# enter a shell with nightly rust, cranelift, clippy, rust-analyzer,
# mold, all system libs and OBAYEBAR_FONT_DIR pre-set
nix develop

# inside the shell
cargo run --bin obayebar
cargo run --bin obayebar-launcher
cargo clippy --all-targets
```

If you'd rather just build the package without setting up a toolchain at
all:

```sh
nix build .#default
nix run .#default
```

A home-manager module is exported as `homeManagerModules.default`. Enable
with `programs.obayebar.enable = true;` and optionally
`programs.obayebar.gitlab = { enable = true; url = "..."; tokenFile = ...; };`.

Building outside Nix is supported but not the happy path: you'll need to
install nightly Rust (with the `rustc-codegen-cranelift-preview` component),
`mold`, and the system libraries listed above yourself.

## CLI

```
obayebar [OPTIONS]

  --gitlab              Show the GitLab todos module on the bar
  --gitlab-url <URL>    Base URL of the GitLab instance
  -h, --help            Print this help
  -V, --version         Print version
```

Persistent settings: `$XDG_CONFIG_HOME/obayebar/config.toml`.

## Status

Single-user project. No release schedule, no support, no plugin system.

Feature requests and pull requests are welcome under one rule: anything that
adds surface area must be **opt-out-able**, so the default experience stays
the same as it is today.

- If the feature has **no measurable performance impact** when disabled (a
  branch on a config field, a dbus subscription that only spins up when
  asked, a UI module hidden by default, etc.), expose it through
  `$XDG_CONFIG_HOME/obayebar/config.toml` (and ideally the home-manager
  module too). The GitLab module is the existing reference implementation —
  see `[gitlab]` in the config and `programs.obayebar.gitlab.*` in the
  flake.
- If the feature has **any** performance impact when merely *compiled in*
  — extra dependency, extra background task, larger binary, longer
  startup — it must be gated behind a Cargo feature flag and be off by
  default. "Slightest" is intentional: I'd rather say no to a feature than
  pay for it on every machine that doesn't use it.

If you're not sure which bucket your feature falls into, open the issue
first and we'll figure it out before you write the patch.

## Credits

- [caelestia-dots/caelestia](https://github.com/caelestia-dots/caelestia) —
  the design language and feature set this bar borrows shamelessly. obayebar
  is a Rust/iced re-implementation of the parts I personally use, not a
  replacement or competitor.
- [waycrate/exwlshelleventloop](https://github.com/waycrate/exwlshelleventloop)
  — `iced_layershell`, without which none of this would compile.

## License

MIT.
