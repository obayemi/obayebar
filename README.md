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

A cargo workspace producing four binaries, of which the bar is the big one — a
left-anchored vertical bar on every Hyprland output, plus satellite layer-shell
windows for popups and panels:

- one bar per connected monitor, automatically respawned when outputs come back
  from sleep / disconnect
- a dbus notification daemon with a stacked popup overlay
- click-to-open settings panels: audio, network, bluetooth, battery / power
  profile, sysinfo, optional GitLab todos

| Binary                | What it does                                                    |
|-----------------------|-----------------------------------------------------------------|
| `obayebar`            | the bar itself                                                  |
| `obayebar-launcher`   | an app-launcher layer surface                                   |
| `obayebar-wallpaper`  | a random wallpaper per monitor, rotated on a timer              |
| `obayebar-lock`       | locks the session, showing the wallpaper the desktop is showing |

Only the first two link iced. The wallpaper renderer talks wlr-layer-shell
directly and the lock screen draws nothing at all, so both build from a shared
crate that has no GUI stack in it — see [Crate layout](#crate-layout).

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

Configuration lives in `$XDG_CONFIG_HOME/obayebar/config.toml` — `[gitlab]`,
`[wallpaper]` and `[lock]`; CLI flags override the file, env vars override
that. A home-manager module ships in `flake.nix`.

## Wallpapers

`obayebar-wallpaper` puts a different random wallpaper on every monitor and
rotates them on a timer. It replaces a pair of fish scripts (`hyprwallp`,
`hyprrandlock`) that shelled out to `hyprctl`, `jq`, `find` and `shuf`, and it
does **not** use hyprpaper — see [Why not hyprpaper](#why-not-hyprpaper).

```toml
# $XDG_CONFIG_HOME/obayebar/config.toml
[wallpaper]
enable = true
directory = "~/Images/wallpapers/enabled"   # default
interval = "30m"                            # "45s", "2h", "1d", or "off"
```

```
obayebar-wallpaper [OPTIONS]

  -d, --directory <DIR>   Override the wallpaper directory
  -i, --interval <SPEC>   Override the rotation interval
      --once              Assign once, write the state file, exit
      --next              Tell the running daemon to rotate now
      --reload            Tell it to re-scan the directory
  -h, --help              Print this help
  -V, --version           Print version
```

Bind `--next` to a key if you want a wallpaper you dislike gone immediately:

```
bind = SUPER, W, exec, obayebar-wallpaper --next
```

### How pictures get picked

- **Files are identified by content, not by extension.** `image::open` and
  friends dispatch on the file extension, which silently misses the two shapes
  a real wallpaper directory actually contains — a `.jpe` file, and one named
  `…wallpaper.jpg.png`. Every candidate is sniffed by magic bytes instead, so a
  mislabelled picture works and a `.txt` renamed to `.png` is skipped rather
  than rendered as nothing.
- **The order is a seeded shuffle with a cursor**, both persisted. That is what
  makes "next" mean something across a restart. The scripts reshuffled from
  scratch on every run, so there was no such thing as *next* and the same
  picture could come back immediately.
- **A monitor never gets the wallpaper it is already showing.** The scripts
  tried this with `find ! -name "$(basename "$CURRENT_WALL")"`, but
  `CURRENT_WALL` was never set, so the filter excluded nothing and was dead
  code.
- **Fewer wallpapers than monitors wraps**, matching the scripts' modulo.
- **Rotation is lockstep**: every monitor changes together, from one cursor.
  Startup and hotplug are *not* rotations — a monitor coming back gets its
  previous wallpaper, and only genuinely new monitors are assigned.

### State

The current selection lives in `$XDG_DATA_HOME/obayebar/wallpapers.json`,
written atomically because the lock screen reads it while the timer may be
writing it. `obayebar-wallpaper` is its only writer.

It is keyed on the monitor **description** (`Dell Inc. DELL U2518D 3C4YP95TBQ5L`),
not the port. Two identical panels differ only by the serial in that string, and
DPMS cycles reshuffle which port is which — a wallpaper remembered against
`DP-9` would be lost the moment the same screen came back as `DP-10`. The data
dir rather than the cache dir, so the desktop comes back looking as it was left
instead of reshuffling on every login.

### Why not hyprpaper

hyprpaper 0.8.4 leaks roughly 4 MB per `wallpaper` IPC request, unbounded: a
fresh daemon driven through ~130 requests grew from 84 MB to 1.55 GB RSS, and it
is per-request rather than per-image — the same picture set repeatedly leaks
just as fast. The `preload`/`unload` verbs that used to bound it are gone in
0.8.4, so the only way to reclaim the memory is to restart hyprpaper, which
wipes every wallpaper with no event to notice it by. For a feature whose entire
purpose is to send that request on a timer, that is disqualifying.

Rendering directly also removes a whole class of bug rather than working around
it. `obayebar-wallpaper` talks wlr-layer-shell through
`smithay-client-toolkit`, whose `create_layer_surface` binds a `wl_output`
**object** — so a wallpaper cannot land on the wrong monitor, and none of the
namespace-verification machinery the bar needs (see [Status](#status)) applies
here. It draws into a `wl_shm` buffer, so it links neither iced nor wgpu and
holds no VRAM.

## Lock screen

`obayebar-lock` locks the session showing the wallpaper the desktop is
currently showing, blurred — not a fresh random one, which is what
`hyprrandlock` did. It reads `wallpapers.json`, generates a hyprlock config
with one `background` block per monitor, and runs hyprlock against it.

```toml
[lock]
enable = true
config = "~/.config/hypr/hyprlock.conf"   # default; your own file, see below
blur_passes = 1
blur_size = 3
```

```
bind = SUPER, O, exec, obayebar-lock
```

`programs.obayebar.lock.idle.enable` sets up hypridle to run it after a
timeout, and before suspend, so the screen is never briefly unlocked on resume.

### Your base config is read, never replaced

`[lock].config` points at *your* hyprlock config and obayebar only appends to a
copy of it. That is deliberate: a config in this repository would not carry the
`auth { fingerprint { … } }` block a real one does, and shipping a template
would silently disable fingerprint unlock.

Two hyprlock behaviours the generated config works around:

- **Widget matching is additive.** A monitor-less `background` plus a
  per-monitor one gives that monitor *two* backgrounds. The scripts relied on
  the appended block happening to be drawn last; the generated blocks carry an
  explicit `zindex` above hyprlock's default instead, so the order is defined.
- **The monitor-less block is kept on purpose.** A screen that wakes *while the
  session is locked* can only be matched by a block with no `monitor` set;
  without one its surface renders transparent. If your base config has no such
  block, `obayebar-lock` warns rather than editing your file.

Since hyprlock parses its config only *after* connecting to Wayland, there is
no way to pre-flight one without locking the screen — hence `--print` to dump
the generated config and `--check` to validate it.

### Why not a native lock screen

Writing the locker in Rust against `ext-session-lock-v1` would be the obvious
move, and it is the one thing here deliberately *not* done. The available iced
binding `delegate_noop!`s `ExtSessionLockV1`, so the protocol's `locked` and
`finished` events are discarded and the client can never learn whether the lock
took effect; `unlock_and_destroy` then fires unconditionally, and the resulting
`invalid_unlock` protocol error lands on an `.expect()`.

The protocol says a compositor "must not unlock the session" when the client
dies, and with `misc:allow_session_lock_restore` off — the default — Hyprland
will not let a replacement take over. A crashed locker therefore means a
machine you can only recover by killing the compositor from a TTY or over SSH,
losing the session. hyprlock is a worse programming model and a much better
failure mode.

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
- **Notification popup is auto-sized.** The popup is pinned to a known output
  and resizes to fit the current notifications, capped at 2/5 of *that*
  monitor's logical height; anything that doesn't fit is collapsed into a
  single "*N more notifications*" entry rather than rendering offscreen
  widgets. It follows the focused monitor by being recreated there, since a
  layer surface can be resized but not moved.
- **Launcher cache.** `obayebar-launcher` persists desktop-entry parsing and
  resolved icon paths to `XDG_CACHE_HOME` and launch frequencies to
  `XDG_DATA_HOME`, so cold start is almost instant after the first run. A cache
  written by an older version is discarded rather than migrated, so the entry
  list is simply rediscovered once after an upgrade.
- **Terminal applications launch in a terminal.** Entries with
  `Terminal=true` (htop, vim, ranger, …) are wrapped in `$TERMINAL`, falling
  back to the first of foot, kitty, alacritty, wezterm, konsole,
  gnome-terminal, xfce4-terminal or xterm found on `PATH`. `Exec` lines run
  through `sh -c`, so quoted arguments, `env VAR=value` prefixes and
  `sh -c "…"` wrapper entries get the argv the desktop-entry spec asks for.
- **Smithay clipboard worker disabled.** No surface in the bar is
  keyboard-interactive, so the upstream always-on clipboard thread is
  switched off via `iced_layershell::disable_clipboard()`. (The launcher,
  which *is* interactive, runs in its own process.)
- **Secrets stored in the kernel keyring.** GitLab tokens go through Secret
  Service when available, falling back to a file in `XDG_CONFIG_HOME`. The
  token never ends up in the Nix store even via the home-manager module.
- **Verified multi-monitor placement.** Bar placement is *observed*, not
  assumed. Each bar is spawned under its own layer-shell namespace
  (`obayebar-bar-N`) and then checked against Hyprland's `j/layers`: if a bar
  landed on a monitor other than the one requested, vanished without a close
  event, or ended up sharing a screen with another bar, it is closed and
  respawned until the compositor agrees. Spawns are serialised one at a time so
  a batch cannot resolve against a stale output-name cache and pile onto one
  screen. A failed IPC query is treated as "unknown" and changes nothing —
  never as "no monitors are connected".

The last bullet describes a behaviour you specifically should not have to
think about — it just works.

> **Upgrading:** every layer surface now carries its own namespace instead of
> the shared `obayebar`. A Hyprland rule matching the old exact name needs to
> become a prefix match:
>
> ```
> layerrule = blur, ^obayebar
> ```
>
> The namespaces are `obayebar-bar-N` (one per bar), `obayebar-panel-<kind>`
> (audio, network, bluetooth, battery, sysinfo, gitlab) and
> `obayebar-notifications`, so rules can also target one kind of surface
> without catching the others.

## Libraries

| Crate                        | Used for                                                  |
|------------------------------|-----------------------------------------------------------|
| `iced` 0.14                  | Reactive UI runtime, wgpu renderer, canvas, lazy widgets  |
| `iced_layershell` 0.19       | wlr-layer-shell integration on top of iced                |
| `smithay-client-toolkit` 0.20| Raw wlr-layer-shell + wl_shm for the wallpaper renderer   |
| `zbus` 5                     | Async dbus for NetworkManager / BlueZ / UPower / SNI / …  |
| `pipewire` 0.10              | Native PipeWire client for audio                          |
| `tokio` 1.x                  | Async runtime, signal/timer plumbing                      |
| `chrono`                     | Time + minute-aligned wakeups                             |
| `nvml-wrapper`               | NVIDIA GPU usage / temperature                            |
| `fuzzy-matcher` (Skim)       | Launcher fuzzy ranking                                    |
| `resvg` + `image`            | Tray / launcher icon decoding                             |
| `reqwest` (rustls + ring)    | GitLab REST API                                           |
| `secret-service`             | Storing the GitLab PAT in the kernel keyring              |
| `serde` + `toml`             | Config file parsing                                       |
| `ab_glyph` + `fontdb`        | Vector text rendering on the workspace canvas             |
| `thiserror`                  | Typed errors across the IPC and rendering paths           |

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

```
obayebar-lock [OPTIONS]

  -c, --config <PATH>   Base hyprlock config to extend
      --no-wallpaper    Lock with the base config unchanged
      --print           Print the generated config and exit, without locking
      --check           Validate and exit non-zero on any problem
      --blur <P>x<S>    Blur passes and size, e.g. 2x5
  -g, --grace <SECS>    Seconds before a password is required
```

`obayebar-wallpaper`'s flags are in [Wallpapers](#wallpapers).

Persistent settings: `$XDG_CONFIG_HOME/obayebar/config.toml`.

## Crate layout

```
crates/obayebar-core        no iced — monitor detection, wallpaper selection, config schema
crates/obayebar             the bar and the launcher (iced + wgpu)
crates/obayebar-wallpaper   wlr-layer-shell + wl_shm renderer
crates/obayebar-lock        generates a hyprlock config and runs it
```

The split is drawn along one line: whether a binary needs a GUI stack.
`obayebar-core` pulls 81 crates against the bar's 1198, which is what lets the
lock screen be a fast, small program and lets the shared logic's tests run
without compiling wgpu.

Run cargo from the workspace root; `cargo test` covers every member. Use
`cargo run -p obayebar-wallpaper` to run one binary in particular.

Two workspace rules worth knowing before adding a crate. `[profile.dev]` and
`cargo-features` must stay in the root manifest — cargo ignores a profile in a
member and only warns, silently losing the cranelift backend. And every member
needs `[lints] workspace = true`: omitting it is **completely silent** and drops
`unsafe_code = "forbid"` along with the whole deny set.

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
