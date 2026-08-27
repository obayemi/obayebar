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

## The four programs

The cargo workspace makes four programs. The bar is the primary program.

| Program              | Function                                                               |
|----------------------|------------------------------------------------------------------------|
| `obayebar`           | Shows one vertical bar on each monitor, with popups, panels and the application launcher. |
| `obayebar-launcher`  | Asks the running bar to show the launcher.                             |
| `obayebar-wallpaper` | Puts a different wallpaper on each monitor, and changes the wallpapers.|
| `obayebar-lock`      | Locks the session, and shows the wallpaper that each monitor shows.    |

Only the bar links iced. The launcher program writes one line to the control
socket of the bar, the wallpaper program speaks wlr-layer-shell directly, and
the lock program draws no pixels. Thus the three small programs build from a
shared crate that contains no GUI stack. Refer to
[Crate layout](#crate-layout).

---

## Quickstart

Hyprland 0.56 or newer is required. 0.56 moved the dispatchers to Lua, and the
bar sends that form (`hl.dsp.focus`), so on an older compositor a click on a
workspace is refused and the bar logs the refusal.

### 1. Install the programs

The flake gives all four programs. Start the bar immediately:

```sh
nix run github:obayemi/obayebar#default
```

Or put the four binaries in `./result/bin`:

```sh
nix build github:obayemi/obayebar#default
```

For a permanent installation on NixOS, refer to
[NixOS and home-manager](#nixos-and-home-manager). For a build without Nix,
refer to [Build from source](#build-from-source).

### 2. Start the programs

```sh
obayebar             # the bar
obayebar-wallpaper   # the wallpaper daemon
obayebar-launcher    # shows the launcher of the running bar
obayebar-lock        # locks the session now
```

Obey these rules:

- Start only one instance of `obayebar`. One instance makes one bar on each
  monitor, and makes a new bar when a monitor comes back.
- Start only one instance of `obayebar-wallpaper`. The program stays in memory
  and changes the wallpapers on the interval. To assign the wallpapers one
  time and exit, give the `--once` flag.
- Start `obayebar-launcher` for each use. The program asks the running bar to
  show the launcher, and exits immediately. A second use hides the launcher.
  The launcher also closes when you start an application, and when you push
  `Esc`. The bar must run: without a bar there is no launcher.
- `obayebar-lock` starts hyprlock and waits. The program exits when you unlock
  the session.
- Stop `mako`, `dunst`, and all other notification daemons. Only one program
  can own the `org.freedesktop.Notifications` dbus name.
- Stop `hyprpaper` before you start `obayebar-wallpaper`. Two wallpaper
  programs fight for the background layer.

### 3. Configure Hyprland

Add these lines to `~/.config/hypr/hyprland.conf`:

```
# start the bar and the wallpaper daemon with the session
exec-once = obayebar
exec-once = obayebar-wallpaper

# keys
bind = SUPER, D, exec, obayebar-launcher
bind = SUPER, W, exec, obayebar-wallpaper --next
bind = SUPER, O, exec, obayebar-lock

# blur all the surfaces of obayebar
layerrule = blur, ^obayebar
```

If home-manager starts the bar and the wallpaper daemon, remove the two
`exec-once` lines. Refer to [NixOS and home-manager](#nixos-and-home-manager).

**Each surface has its own layer-shell namespace.** The namespaces are
`obayebar-bar-N` (one for each bar), `obayebar-panel-<kind>` (audio, network,
bluetooth, battery, sysinfo, gitlab), `obayebar-notifications` and
`obayebar-launcher`. Thus a rule must match a prefix, as in the example above.
A rule that matches the exact name `obayebar` matches no surface. A rule can
also match one kind of surface, and not the other kinds.

### 4. Configure obayebar

The three programs read `$XDG_CONFIG_HOME/obayebar/config.toml`, usually
`~/.config/obayebar/config.toml`. All the sections are optional. An unknown
key makes a parse error. The program then writes a warning, and uses the
default values for the full file.

```toml
[gitlab]
enable = true                               # show the GitLab todos module
url = "https://gitlab.example.com"          # default: https://gitlab.com

[wallpaper]
enable = true                               # for the home-manager module only
directory = "~/Images/wallpapers/enabled"   # default
interval = "30m"                            # "45s", "2h", "1d", or "off"

[lock]
enable = true                               # for the home-manager module only
config = "~/.config/hypr/hyprlock.conf"     # default; your own file
blur_passes = 1                             # default
blur_size = 3                               # default
```

The precedence for each field is: command-line flag, then environment
variable, then configuration file, then the default value.

`[gitlab].enable` controls the module on the bar. `[wallpaper].enable` and
`[lock].enable` control the systemd units of the home-manager module. The
`obayebar-wallpaper` and `obayebar-lock` programs do not read the two `enable`
keys: a program that you start manually always operates.

The GitLab token does not go in this file. The bar reads the token from the
`OBAYEBAR_GITLAB_TOKEN` environment variable, then from Secret Service, then
from `~/.config/obayebar/gitlab_token`.

---

## NixOS and home-manager

The flake exports `homeManagerModules.default`. The module installs the
programs, writes the configuration file, and starts the systemd user units.

```nix
# In flake.nix: inputs.obayebar.url = "github:obayemi/obayebar";

# Then, in your home-manager configuration (`inputs` via `extraSpecialArgs`):
{ config, inputs, ... }:
{
  imports = [ inputs.obayebar.homeManagerModules.default ];

  programs.obayebar = {
    enable = true;

    gitlab = {
      enable = true;
      url = "https://gitlab.example.com";
      tokenFile = "/run/secrets/obayebar-gitlab-token";
    };

    wallpaper = {
      enable = true;
      directory = "${config.home.homeDirectory}/Images/wallpapers/enabled";
      interval = "30m";
    };

    lock = {
      enable = true;
      config = "${config.xdg.configHome}/hypr/hyprlock.conf";
      blurPasses = 2;
      blurSize = 5;
      idle = {
        enable = true;
        timeout = 300;
      };
    };
  };
}
```

The path options take an absolute path. Nix expands neither `~` nor `$HOME`
in a string, and the module writes the string as it is. Thus build the path
from home-manager itself: `config.home.homeDirectory` for the home, and
`config.xdg.configHome`, `config.xdg.dataHome` or `config.xdg.cacheHome` for
the XDG directories. The `~` form works only in a hand-written
`config.toml`, which obayebar reads itself.

### What the module does

- The module adds the package to `home.packages`. Thus all four programs are
  on the `PATH`.
- The module writes `~/.config/obayebar/config.toml` from the options. The
  module writes only the sections that you enable. If you enable no section,
  the module writes no file.
- The module adds the `obayebar` systemd user service. The service starts with
  `programs.obayebar.systemd.target`, and starts again after a failure.
- If `wallpaper.enable` is true, the module adds a second service,
  `obayebar-wallpaper`. The service is independent of the bar: a failure of
  the bar does not remove the wallpapers, and a restart of the bar does not
  make the wallpapers flash.
- `systemctl --user reload obayebar-wallpaper` runs `obayebar-wallpaper
  --reload`. The daemon then reads the directory again, and does not change
  the picture on the screen.
- If `lock.enable` and `lock.idle.enable` are true, the module configures
  hypridle. hypridle then runs `obayebar-lock` after the timeout, and runs
  `obayebar-lock --detach` before the machine goes to sleep.
- The module reads `gitlab.tokenFile` at start, and puts the contents in
  `OBAYEBAR_GITLAB_TOKEN`. The module reads the path at run time. Thus the
  token does not go into the Nix store.

### Options

| Option                             | Type    | Default                        | Function                                                     |
|------------------------------------|---------|--------------------------------|--------------------------------------------------------------|
| `programs.obayebar.enable`         | bool    | `false`                        | Install obayebar and start the bar.                          |
| `programs.obayebar.package`        | package | the package of this flake      | The package to install.                                      |
| `systemd.enable`                   | bool    | `true`                         | Add the systemd user services.                               |
| `systemd.target`                   | str     | `config.wayland.systemd.target`| The target that starts the services.                         |
| `gitlab.enable`                    | bool    | `false`                        | Show the GitLab todos panel.                                 |
| `gitlab.url`                       | str     | `null`                         | The GitLab instance. `null` gives `https://gitlab.com`.      |
| `gitlab.tokenFile`                 | path    | `null`                         | A file that contains the personal access token.              |
| `wallpaper.enable`                 | bool    | `false`                        | Start the wallpaper daemon.                                  |
| `wallpaper.directory`              | path    | `null`                         | `null` gives `~/Images/wallpapers/enabled`.                  |
| `wallpaper.interval`               | str     | `null`                         | `null` gives `30m`. Use `off` to select one time.            |
| `lock.enable`                      | bool    | `false`                        | Enable the lock screen configuration.                        |
| `lock.config`                      | path    | `null`                         | Your hyprlock config. `null` gives `~/.config/hypr/hyprlock.conf`. |
| `lock.blurPasses`                  | int     | `null`                         | `null` gives 1.                                              |
| `lock.blurSize`                    | int     | `null`                         | `null` gives 3.                                              |
| `lock.idle.enable`                 | bool    | `false`                        | Configure hypridle to lock the session.                      |
| `lock.idle.timeout`                | int     | `300`                          | Seconds of inactivity before the lock.                       |

An overlay is also available. The overlay adds `obayebar` to `pkgs`.

---

## Modules on the bar

| Module          | Source                                   | Notes                                                                    |
|-----------------|------------------------------------------|--------------------------------------------------------------------------|
| Workspaces      | Hyprland IPC (`j/workspaces`, socket2)   | One set per monitor. A click focuses one. A spring moves the indicator.  |
| Active window   | Hyprland IPC (`activewindow` event)      | Shows the class and the title. The bar draws the text vertically.       |
| System tray     | StatusNotifierItem (dbus)                | A click activates the item. The bar keeps the icons in a cache.         |
| GitLab todos    | GitLab REST API + Secret Service keyring | Off by default. Use `--gitlab`, the config file, or the Nix option.     |
| Clock           | local time tick                          | Shows the local time.                                                   |
| Audio           | PipeWire (native, with `pipewire-rs`)    | Shows the volume. The panel has sliders, mute, and sink selection.      |
| Network         | NetworkManager (dbus)                    | The panel shows the Wi-Fi list, and connects or disconnects.            |
| Bluetooth       | BlueZ (dbus)                             | The panel starts the adapter, finds devices, and forgets devices.       |
| Battery / power | UPower + `power-profiles-daemon` (dbus)  | Shows the percentage. The panel changes the power profile.              |
| Sysinfo         | `/proc`, NVML                            | Shows CPU, GPU, RAM, and network rates. The color changes at a limit.   |
| Notifications   | `org.freedesktop.Notifications` (dbus)   | Replaces `mako` and `dunst`. Maximum height is 2/5 of the monitor.      |

## Command-line reference

```
obayebar [OPTIONS]

  --gitlab              Show the GitLab todos module on the bar
  --gitlab-url <URL>    Base URL of the GitLab instance
  -h, --help            Print this help
  -V, --version         Print version
```

```
obayebar-wallpaper [OPTIONS]

  -d, --directory <DIR>   Directory to pick wallpapers from
  -i, --interval <SPEC>   Rotation interval: 45s, 30m, 2h, 1d, or off
      --once              Assign once, write the state file, and exit
      --next              Ask the running daemon to rotate now
      --reload            Ask it to re-scan the wallpaper directory
  -h, --help              Print this help
  -V, --version           Print version
```

```
obayebar-lock [OPTIONS]

  -c, --config <PATH>     Base hyprlock config to extend
      --state <PATH>      Wallpaper state file to read
      --no-wallpaper      Lock with the base config unchanged
      --print             Print the generated config and exit, without locking
      --check             Validate and exit non-zero on any problem
      --blur <P>x<S>      Blur passes and size, e.g. 2x5
  -g, --grace <SECS>      Seconds before a password is required
      --detach            Do not wait for hyprlock to exit
      --no-scope          Do not wrap hyprlock in its own systemd scope
  -h, --help              Print this help
  -V, --version           Print version
```

`obayebar-launcher` takes `--help` and `--version`, and nothing else. Each
other use shows or hides the launcher of the running bar.

### Logging

Both daemons log at `info` by default: startup and shutdown, monitor and
service changes, and every command received over a bus or socket. `RUST_LOG`
overrides that in either direction, and takes the usual per-target filters.

```
RUST_LOG=debug obayebar                       # more
RUST_LOG=error obayebar-wallpaper             # less
RUST_LOG=obayebar::services=debug obayebar    # one module
```

`obayebar-lock` and `obayebar-launcher` are one-shot commands that report
their own failures on stderr, so they stay at `error` unless `RUST_LOG`
says otherwise.

## Wallpapers

`obayebar-wallpaper` puts a different wallpaper on each monitor, and changes
the wallpapers on a timer. The program replaces two fish scripts (`hyprwallp`
and `hyprrandlock`) that called `hyprctl`, `jq`, `find` and `shuf`. The
program does **not** use hyprpaper. Refer to
[Why not hyprpaper](#why-not-hyprpaper).

### How the program selects a picture

- **The content identifies a file, not the extension.** `image::open` and the
  related functions use the extension. Thus these functions do not read the
  two shapes that a real wallpaper directory contains: a `.jpe` file, and a
  file with the name `…wallpaper.jpg.png`. The program examines the magic
  bytes of each candidate file instead. Thus a file with an incorrect name
  operates correctly, and the program rejects a `.txt` file that has the new
  name `.png`.
- **The order is a shuffle with a seed and a cursor.** The program keeps the
  seed and the cursor on disk. Thus *next* has a meaning after a restart. The
  scripts made a new shuffle at each run, thus the scripts had no *next*, and
  the same picture could come back immediately.
- **A monitor does not receive the wallpaper that the monitor shows.** The
  scripts tried this filter with `find ! -name "$(basename "$CURRENT_WALL")"`.
  But no code set `CURRENT_WALL`, thus the filter removed no file, and the
  filter was dead code.
- **The program uses a wallpaper again when the wallpapers are not
  sufficient.** If the monitors are more than the wallpapers, the program uses
  a modulo, as the scripts did.
- **All the monitors change together**, from one cursor. The start of the
  program and a new monitor are not changes: a monitor that comes back
  receives the previous wallpaper of that monitor, and the program assigns a
  wallpaper only to a fully new monitor.

### The state file

The program writes the current selection to
`$XDG_DATA_HOME/obayebar/wallpapers.json`. The write is atomic, because the
lock screen reads the file while the timer can write the file.
`obayebar-wallpaper` is the only writer.

The key is the **description** of the monitor
(`Dell Inc. DELL U2518D 3C4YP95TBQ5L`), not the port. Two equal panels differ
only by the serial number in that string. Also, a DPMS cycle can change which
port has which monitor. Thus a wallpaper that the program remembered against
`DP-9` would be lost when the same monitor came back as `DP-10`. The file is
in the data directory, not in the cache directory. Thus the desktop comes back
in the previous condition, and does not shuffle at each login.

### The time to change a wallpaper

A release build needs approximately **50 ms** for each monitor: approximately
25 ms to decode the JPEG, 20 ms to scale the image, and 3 ms to write the
buffer. Two changes give this result, and measurements found the two changes.
The program logs the time of each phase at the `debug` level. Thus you can
examine the times on your own hardware.

**The buffers have the size of the panel, not the size of the surface.** The
obvious calculation is the logical size of the layer surface multiplied by the
integer scale of the monitor. That calculation made a 3840×2560 buffer for a
2256×1504 panel. That buffer has three times more pixels than the monitor can
show, and the compositor then made the pixels smaller again. `wp_viewporter`
separates the buffer size from the surface size. Thus the buffer can have the
real mode of the panel. This change alone removed one half of the time.

**The program scales with `fast_image_resize`, not with the `image` crate.**
The scale operation used approximately 92% of the remaining time, because the
`image` crate uses scalar code. The SIMD code is approximately 20 times
faster, and this change makes a wallpaper change immediate. The filter has
almost no effect: CatmullRom is only 15% faster than Lanczos3. Thus the
program keeps Lanczos3 and the better quality.

A **debug build needs approximately 14 s for each wallpaper**. Almost all of
that time is the unoptimized scale operation. Add `--release` when you examine
the speed.

### Why not hyprpaper

hyprpaper 0.8.4 loses approximately 4 MB of memory at each `wallpaper` IPC
request, with no limit. A new daemon that received approximately 130 requests
increased from 84 MB to 1.55 GB of RSS. The loss is for each request, not for
each image: the same set of pictures loses memory at the same speed. Version
0.8.4 does not have the `preload` and `unload` commands that limited the
memory before. Thus only a restart of hyprpaper releases the memory, and a
restart removes each wallpaper with no event to detect. A program that sends
that request on a timer cannot accept this behavior.

A direct draw also removes a full class of defect. `obayebar-wallpaper` speaks
wlr-layer-shell through `smithay-client-toolkit`, and
`create_layer_surface` binds a `wl_output` **object**. Thus a wallpaper cannot
go to the incorrect monitor, and the namespace verification of the bar (refer
to [Why obayebar is light](#why-obayebar-is-light)) is not necessary here. The
program draws into a `wl_shm` buffer. Thus the program links neither iced nor
wgpu, and holds no VRAM.

## Lock screen

`obayebar-lock` locks the session and shows the wallpaper that each monitor
currently shows, with a blur. The program does not select a new random
picture, as `hyprrandlock` did. The program reads `wallpapers.json`, makes a
hyprlock config with one `background` block for each monitor, and starts
hyprlock with that config.

### The program reads your base config, and does not replace your base config

`[lock].config` points to *your* hyprlock config, and obayebar adds text only
to a copy of that file. This behavior is intentional: a config in this
repository would not contain the `auth { fingerprint { … } }` block that a real
config contains. A template would then disable the fingerprint unlock, and
would give no message.

The generated config prevents two behaviors of hyprlock:

- **hyprlock adds the widgets together.** A `background` block with no monitor,
  and a second block for one monitor, give that monitor *two* backgrounds. The
  scripts depended on the sequence of the blocks. The generated blocks contain
  an explicit `zindex` above the default of hyprlock instead. Thus the
  sequence is known.
- **The block with no monitor stays, and this is intentional.** A monitor that
  comes back *while the session is locked* can match only a block with no
  `monitor` key. Without such a block, the surface of that monitor is
  transparent. If your base config has no such block, `obayebar-lock` gives a
  warning, and does not change your file.

hyprlock reads the config only *after* the connection to Wayland. Thus no
method can test a config without a lock of the screen. For this reason,
`--print` writes the generated config, and `--check` examines the generated
config.

### Why not a native lock screen

A locker in Rust on `ext-session-lock-v1` is the obvious solution, and this is
the one item that this repository intentionally does *not* do. The available
iced binding applies `delegate_noop!` to `ExtSessionLockV1`. Thus the binding
discards the `locked` and `finished` events of the protocol, and the client
can never know if the lock is effective. `unlock_and_destroy` then operates in
all conditions, and the `invalid_unlock` protocol error goes to an
`.expect()`.

The protocol says that a compositor "must not unlock the session" when the
client stops. Also, with `misc:allow_session_lock_restore` off (the default),
Hyprland does not let a second client take control. Thus a locker that stops
gives a machine that you can recover only from a TTY or through SSH, and you
lose the session. hyprlock is a worse programming model, and a much better
failure mode.

## Why obayebar is light

The primary goal of this program is to **not** be caelestia-shell. These
decisions come from that goal:

- **No QtQuick, no JavaScript, no shell runtime.** obayebar is a Rust binary on
  [`iced`](https://iced.rs) and
  [`iced_layershell`](https://github.com/waycrate/exwlshelleventloop), which
  speak wlr-layer-shell directly. There is no QML interpreter, no V8, and no
  Qt scene graph.
- **A wgpu renderer with cached widgets.** The bar draws the workspace
  indicators on a `canvas::Cache`, and clears the cache only at a true change
  of the state. The clock, the status and the tray sections are in an
  `iced::widget::lazy` with a manual cache key. Thus a CPU or RAM value with a
  change of 0.1 % does not make a new widget tree. The spring animation runs
  at 60 Hz only *while the animation moves*. When nothing moves, the bar is
  fully idle: no wake-ups, and no draws.
- **All the event sources push, and the bar does not poll.**
  - Hyprland: one permanent `socket2` connection. The bar reads the connection
    line by line. Only an event that changes the screen (a workspace, a window,
    or a monitor) makes a refresh. The bar discards the high-frequency events
    `activewindowv2` and `windowtitle` before the UI thread wakes.
  - The dbus services (network, bluetooth, notifications, battery,
    power-profiles, upower, gitlab, tray) all use signal subscriptions
    through `zbus`.
  - The audio data comes directly from the native PipeWire protocol
    (`pipewire-rs`), not from `pactl`, and not from a poll of `pavucontrol`.
- **The clock wakes at the minute, not at each frame.** The clock uses a special
  timer subscription (`services::timers::clock_stream`). The timer wakes
  exactly at the next minute, and at the next expiry of a notification. The
  timer never uses a constant interval.
- **The notification popup has an automatic size.** The popup stays on one
  monitor, and changes size to fit the current notifications. The maximum is
  2/5 of the logical height of *that* monitor. The popup collapses the
  remaining notifications into one "*N more notifications*" entry, and does
  not draw widgets off the screen. To follow the focused monitor, the bar
  makes the popup again on that monitor, because a layer surface can change
  size but cannot move.
- **The launcher is a surface of the bar, and not a program.** A use of the old
  launcher started a program of 30 MB, made a wgpu device, and read every
  `.desktop` file again, all before the first pixel. The bar holds the entries
  and the icons already. Thus a use costs one layer surface and one frame,
  approximately 5 ms. `obayebar-launcher` writes `launcher-toggle` to
  `$XDG_RUNTIME_DIR/obayebar/bar.sock` and exits in under 1 ms.
- **The application list follows the file system.** The bar reads the
  application directories one time at the start, and then only when inotify
  reports a change. A group of changes waits 400 ms. Thus an installation and a
  removal both appear within one second, and nothing else makes a read. A
  `nixos-rebuild` does not change the entries: the rebuild replaces a *symlink*
  on the path to the entries, and inotify follows a symlink at the time of the
  watch. Thus the bar also watches the directory of each symlink on that path.
- **The bar reads a desktop entry to the specification.**
  `freedesktop-desktop-entry` reads the files, and `freedesktop-icons` finds
  the icons. Thus the bar honors a translated name, `OnlyShowIn`, `NotShowIn`,
  `TryExec`, a desktop ID from a subdirectory, and the inheritance of an icon
  theme. The `Exec` line stays here: `parse_exec` of that library divides on
  each space, and destroys a quoted argument that contains a space.
- **The launcher writes the icons one time.** The bar writes each icon to
  `XDG_CACHE_HOME` as raw RGBA of 24×24, with the name of the source file. A
  read of that file needs no decode. The launch counts go to `XDG_DATA_HOME`.
  The bar removes the icons of an application that you remove.
- **A terminal application starts in a terminal.** The launcher puts an entry
  with `Terminal=true` (htop, vim, ranger, …) in `$TERMINAL`. If `$TERMINAL`
  is not set, the launcher uses the first of foot, kitty, alacritty, wezterm,
  konsole, gnome-terminal, xfce4-terminal or xterm on the `PATH`. The launcher
  starts each `Exec` line through `sh -c`. Thus a quoted argument, an
  `env VAR=value` prefix, and an `sh -c "…"` entry all receive the argv of the
  desktop-entry specification.
- **One surface receives the keyboard.** The launcher takes an exclusive
  keyboard grab, and every other surface takes none. The bar reads `Esc`, the
  arrows and `Enter` from the raw event stream, because the search field has
  the focus and takes those keys. That grab also keeps the smithay clipboard
  worker on: `Ctrl+V` in the search field is a true paste.
- **The kernel keyring holds the secrets.** A GitLab token goes through Secret
  Service. If Secret Service is not available, the token goes to a file in
  `XDG_CONFIG_HOME`. The token never goes into the Nix store, also through the
  home-manager module.
- **The bar verifies the position on each monitor.** The bar *examines* the
  position, and does not assume the position. The bar starts each surface with
  its own layer-shell namespace (`obayebar-bar-N`), and then compares the
  surface with the `j/layers` data of Hyprland. The bar closes a surface and
  starts a new surface in four conditions: the bar is on an incorrect
  monitor, the bar stops with no close event, two bars are on one monitor, or
  the surface does not appear within two seconds. The bar continues until the
  compositor agrees. The bar starts the surfaces one at a time. Thus a group
  of surfaces cannot use an old output-name cache, and cannot collect on one
  monitor. If an IPC query fails, the bar reads the result as "unknown" and
  changes nothing. The bar never reads a failed query as "no monitor is
  connected".
- **A close is a request, and the bar verifies the request.** The bar keeps
  the window of a surface it closes until `j/layers` reports the surface is
  gone, and asks again if the surface stays. A surface the bar merely forgets
  is a surface no code can reach: a slow surface that maps after the bar gives
  up on it belongs to nobody, holds an exclusive zone, and pushes the real bar
  aside. That is the shape of the extra bars a dock plug and unplug produced.
  The two-second window is wall-clock, not a count of passes, and one pass
  runs at a time — a hotplug sends a burst of events, and a burst of passes
  used to spend the whole window in a fraction of a second.

The last item is a behavior that you must not think about. The behavior
operates.

## Libraries

| Crate                        | Used for                                                  |
|------------------------------|-----------------------------------------------------------|
| `iced` 0.14                  | Reactive UI runtime, wgpu renderer, canvas, lazy widgets  |
| `iced_layershell` 0.19       | wlr-layer-shell integration on top of iced                |
| `smithay-client-toolkit` 0.20| Raw wlr-layer-shell + wl_shm for the wallpaper renderer   |
| `fast_image_resize` 6        | SIMD wallpaper scaling                                    |
| `zbus` 5                     | Async dbus for NetworkManager / BlueZ / UPower / SNI / …  |
| `pipewire` 0.10              | Native PipeWire client for audio                          |
| `tokio` 1.x                  | Async runtime, signal and timer plumbing                  |
| `chrono`                     | Time and minute-aligned wakeups                           |
| `nvml-wrapper`               | NVIDIA GPU usage and temperature                          |
| `fuzzy-matcher` (Skim)       | Launcher fuzzy ranking                                    |
| `resvg` + `image`            | Tray and launcher icon decoding                           |
| `freedesktop-desktop-entry`  | Reading of a `.desktop` file to the specification         |
| `freedesktop-icons`          | Icon lookup, with the inheritance of a theme              |
| `inotify`                    | Watch of the application directories                      |
| `reqwest` (rustls + ring)    | GitLab REST API                                           |
| `secret-service`             | Storage of the GitLab PAT in the kernel keyring           |
| `serde` + `toml`             | Config file parsing                                       |
| `ab_glyph` + `fontdb`        | Vector text on the workspace canvas                       |
| `thiserror`                  | Typed errors on the IPC and rendering paths               |

## Build from source

The project needs some components that a usual distribution toolchain does not
have:

- **Nightly Rust** — necessary for `cargo-features = ["codegen-backend"]` and
  the `rustc-codegen-cranelift-preview` component, which is the dev codegen
  backend. The Cranelift backend makes the incremental dev builds fast. A
  release build continues to use LLVM.
- **`mold`** — the linker. iced, wgpu and `pipewire-rs` give many object
  files. `mold` needs approximately one half of the time of `lld`, and much
  less time than the default GNU `ld`. The `.cargo` directory has the
  configuration.
- **System libraries** — `wayland`, `libxkbcommon`, `vulkan-loader`,
  `fontconfig`, `pipewire`, and also `pkg-config`, `clang` and `libclang` at
  build time.
- **The Material Symbols font** — the bar finds the font at run time with
  `OBAYEBAR_FONT_DIR`.

Thus **the recommended method is the Nix dev shell**:

```sh
# a shell with nightly rust, cranelift, clippy, rust-analyzer, mold,
# all the system libraries, and OBAYEBAR_FONT_DIR
nix develop

# in the shell
cargo run --bin obayebar
cargo clippy --all-targets
```

A build without Nix is possible, but is not the recommended method. You must
install nightly Rust (with the `rustc-codegen-cranelift-preview` component),
`mold`, and the system libraries in the list above.

## Crate layout

```
crates/obayebar-core        no iced — monitor detection, wallpaper selection, config, control sockets
crates/obayebar             the bar, and the launcher that the bar draws (iced + wgpu)
crates/obayebar-launcher    one line to the socket of the bar, for a key binding
crates/obayebar-wallpaper   wlr-layer-shell + wl_shm renderer
crates/obayebar-lock        generates a hyprlock config and runs it
```

One line divides the crates: a binary that needs a GUI stack, and a binary
that does not. `obayebar-core` pulls 81 crates, and the bar pulls 1198. Thus
the lock screen is a small and fast program, and the tests of the shared code
run without a build of wgpu.

Run cargo from the root of the workspace. `cargo test` covers each member. To
run one program, use `cargo run -p obayebar-wallpaper`.

Two rules of the workspace are important before you add a crate. First,
`[profile.dev]` and `cargo-features` must stay in the root manifest: cargo
ignores a profile in a member, gives only a warning, and the cranelift backend
is lost. Second, each member needs `[lints] workspace = true`. A member
without that key gives no message, and loses `unsafe_code = "forbid"` and the
full deny set.

## Status

This is a single-user project. There is no release schedule, no support, and
no plugin system.

Feature requests and pull requests are welcome, with one rule: a feature that
adds surface area must have a control to disable the feature. Thus the default
experience stays the same as today.

- **No measurable effect on the performance when the feature is off.**
  Examples: a branch on a config field, a dbus subscription that starts only on
  a request, or a UI module that is hidden by default. Put the control in
  `$XDG_CONFIG_HOME/obayebar/config.toml`, and also in the home-manager module.
  The GitLab module is the reference example. Refer to `[gitlab]` in the
  config, and to `programs.obayebar.gitlab.*` in the flake.
- **Any effect on the performance when the compiler includes the feature.**
  Examples: an added dependency, an added background task, a larger binary, or
  a longer start. Put the feature behind a Cargo feature flag, and make the
  flag off by default. The word "any" is intentional. It is better to refuse a
  feature than to pay for that feature on each machine that does not use the
  feature.

If you do not know the correct group for your feature, open the issue first.
We will find the answer before you write the patch.

## Credits

- [caelestia-dots/caelestia](https://github.com/caelestia-dots/caelestia) —
  the design language and feature set this bar borrows shamelessly. obayebar
  is a Rust/iced re-implementation of the parts I personally use, not a
  replacement or competitor.
- [waycrate/exwlshelleventloop](https://github.com/waycrate/exwlshelleventloop)
  — `iced_layershell`, without which none of this would compile.

## License

MIT.
