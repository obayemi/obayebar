# Home Manager module for obayebar.
#
# Imported from flake.nix, which passes the flake's `self` so the
# default package resolves from the flake outputs — this file is not
# usable standalone.
{ self }:
{ config, pkgs, lib, ... }:
let
  inherit (pkgs.stdenv.hostPlatform) system;
  cfg = config.programs.obayebar;

  tomlFormat = pkgs.formats.toml { };

  gitlabAttrs =
    lib.optionalAttrs cfg.gitlab.enable { enable = true; }
    // lib.optionalAttrs (cfg.gitlab.url != null) { inherit (cfg.gitlab) url; };

  wallpaperAttrs =
    lib.optionalAttrs cfg.wallpaper.enable { enable = true; }
    // lib.optionalAttrs (cfg.wallpaper.directory != null)
      { directory = toString cfg.wallpaper.directory; }
    // lib.optionalAttrs (cfg.wallpaper.interval != null) { inherit (cfg.wallpaper) interval; };

  lockAttrs =
    lib.optionalAttrs cfg.lock.enable { enable = true; }
    // lib.optionalAttrs (cfg.lock.config != null) { config = toString cfg.lock.config; }
    // lib.optionalAttrs (cfg.lock.blurPasses != null) { blur_passes = cfg.lock.blurPasses; }
    // lib.optionalAttrs (cfg.lock.blurSize != null) { blur_size = cfg.lock.blurSize; };

  # The name of the slice unit, without the suffix, as
  # `systemd.user.slices.<name>` wants it.
  sliceUnit = lib.removeSuffix ".slice" cfg.systemd.slice;

  # Written only when it differs from the default the binaries already use,
  # so the common case leaves config.toml alone.
  spawnAttrs = lib.optionalAttrs (cfg.systemd.slice != "app-obayebar.slice") {
    inherit (cfg.systemd) slice;
  };

  # The union of every section, not just GitLab's. Gating on one feature's
  # attrs meant a wallpaper-only configuration produced no config.toml at all,
  # with no warning — the file simply was not written.
  settings =
    lib.optionalAttrs (gitlabAttrs != { }) { gitlab = gitlabAttrs; }
    // lib.optionalAttrs (wallpaperAttrs != { }) { wallpaper = wallpaperAttrs; }
    // lib.optionalAttrs (lockAttrs != { }) { lock = lockAttrs; }
    // lib.optionalAttrs (spawnAttrs != { }) { spawn = spawnAttrs; };

  hasConfig = settings != { };

  lockCmd = "${cfg.package}/bin/obayebar-lock";

  # --replace: a hyprlock that hung after an unlock keeps its scope up, and
  # a plain refusal would leave the session unlocked for as long as it does.
  # Only the paths hypridle drives itself use it; the user's own keybind
  # keeps refusing.
  replacingLockCmd = "${lockCmd} --replace";

  # The session's own hyprctl, not one pinned into this closure: a hyprctl
  # built from a different Hyprland speaks a different IPC version to the
  # running compositor. Falls back to the PATH when Hyprland is configured
  # outside home-manager.
  hyprland = config.wayland.windowManager.hyprland or { };
  hyprctl =
    if hyprland.enable or false then
      "${hyprland.finalPackage or hyprland.package}/bin/hyprctl"
    else
      "hyprctl";

  # Hyprland 0.56 reads dispatch arguments as Lua, where the pre-0.56
  # `dispatch dpms off` is a syntax error and the screens simply stay lit.
  dpms = state: "${hyprctl} dispatch 'hl.dsp.dpms(\"${state}\")'";

  # Two independent listeners rather than one that locks and blanks: they
  # fire at different times, and either one alone is a valid setup.
  idleListeners =
    lib.optional (cfg.lock.enable && cfg.idle.lockTimeout != null) {
      timeout = cfg.idle.lockTimeout;
      on-timeout = replacingLockCmd;
    }
    ++ lib.optional (cfg.idle.screenOffTimeout != null) {
      timeout = cfg.idle.screenOffTimeout;
      on-timeout = dpms "off";
      on-resume = dpms "on";
    };

  execStart =
    if cfg.gitlab.tokenFile == null then
      "${cfg.package}/bin/obayebar"
    else
      let
        tokenPath = lib.escapeShellArg (toString cfg.gitlab.tokenFile);
        wrapper = pkgs.writeShellScript "obayebar-with-token" ''
          if [ -r ${tokenPath} ]; then
            OBAYEBAR_GITLAB_TOKEN="$(cat ${tokenPath})"
            export OBAYEBAR_GITLAB_TOKEN
          fi
          exec ${cfg.package}/bin/obayebar
        '';
      in toString wrapper;
in {
  options.programs.obayebar = with lib; {
    enable = mkEnableOption "obayebar Wayland status bar";

    package = mkOption {
      type = types.package;
      default = self.packages.${system}.default;
      description = "The obayebar package to use.";
    };

    systemd = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = "Whether to enable the systemd user service for obayebar.";
      };

      target = mkOption {
        type = types.str;
        default = config.wayland.systemd.target;
        description = "The systemd target that will automatically start obayebar.";
      };

      slice = mkOption {
        type = types.strMatching "[A-Za-z0-9_.:]+(-[A-Za-z0-9_.:]+)*\\.slice";
        default = "app-obayebar.slice";
        example = "app-obayebar-launched.slice";
        description = ''
          The systemd slice that every program launched from the bar is
          put in. The module declares that slice, and writes the name to
          `[spawn] slice` in config.toml, so the bar and the lock screen
          agree on it.

          A dash separates the levels of the tree, thus
          app-obayebar.slice sits under app.slice, and `app-` is the
          prefix that systemd reserves for the applications of a user.
          Name a slice of obayebar's own here, and not one that another
          unit already defines: the module writes the unit file.
        '';
      };

      managedOom = mkOption {
        type = types.bool;
        default = false;
        description = ''
          Let systemd-oomd kill programs launched from the bar when the
          session runs out of memory. They all live in the
          app-obayebar.slice cgroup, so oomd can shed the whole slice
          while leaving the session's own services alone.

          Off by default: closing someone's browser without warning is
          a surprise, and it does nothing at all unless systemd-oomd is
          running. Turn it on to make that slice the first thing a
          session under memory pressure gives up.
        '';
      };
    };

    gitlab = {
      enable = mkEnableOption "the GitLab todos panel";

      url = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "https://gitlab.example.com";
        description = ''
          Base URL of the GitLab instance. When null, falls back to
          OBAYEBAR_GITLAB_URL if set, then https://gitlab.com.
        '';
      };

      tokenFile = mkOption {
        type = types.nullOr types.path;
        default = null;
        example = "/run/secrets/obayebar-gitlab-token";
        description = ''
          Optional runtime path to a file containing the GitLab PAT.
          When set, the systemd unit reads the file at start and
          exports its contents as OBAYEBAR_GITLAB_TOKEN. The path is
          read at runtime, so the secret never enters the Nix store.
          Leave null to keep the default keyring / on-disk
          ~/.config/obayebar/gitlab_token resolution.
        '';
      };
    };

    wallpaper = {
      enable = mkEnableOption "the per-monitor wallpaper renderer";

      directory = mkOption {
        type = types.nullOr types.path;
        default = null;
        example = lib.literalExpression ''"''${config.home.homeDirectory}/Images/wallpapers/enabled"'';
        description = ''
          Where to look for wallpapers. When null, obayebar-wallpaper uses
          ~/Images/wallpapers/enabled.
        '';
      };

      interval = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "30m";
        description = ''
          How often to rotate: 45s, 30m, 2h, 1d, or "off" to pick once and
          leave it. When null, obayebar-wallpaper uses 30m.
        '';
      };
    };

    lock = {
      enable = mkEnableOption "the hyprlock-based lock screen";

      config = mkOption {
        type = types.nullOr types.path;
        default = null;
        example = lib.literalExpression ''"''${config.xdg.configHome}/hypr/hyprlock.conf"'';
        description = ''
          Base hyprlock config to extend with one background per monitor.
          Deliberately a path to your own file rather than a generated one:
          it carries things obayebar does not model, such as an
          auth{fingerprint{...}} block, and replacing it would silently
          disable fingerprint unlock. When null, ~/.config/hypr/hyprlock.conf.
        '';
      };

      blurPasses = mkOption {
        type = types.nullOr types.int;
        default = null;
        example = 2;
        description = "Blur passes on the generated backgrounds (default 1).";
      };

      blurSize = mkOption {
        type = types.nullOr types.int;
        default = null;
        example = 5;
        description = "Blur size on the generated backgrounds (default 3).";
      };
    };

    idle = {
      enable = mkEnableOption "hypridle, acting on inactivity";

      lockTimeout = mkOption {
        type = types.nullOr types.int;
        default = 300;
        description = ''
          Seconds of inactivity before the session locks. Needs
          lock.enable, which is what supplies the locker. Null never locks
          on a timeout — the session still locks before sleep.
        '';
      };

      screenOffTimeout = mkOption {
        type = types.nullOr types.int;
        default = 600;
        description = ''
          Seconds of inactivity before the monitors turn off. Null leaves
          them on. Independent of lock.enable: a screen that blanks
          without locking is a legitimate configuration, and a screen that
          locks then blanks is the default one.
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    xdg.configFile."obayebar/config.toml" = lib.mkIf hasConfig {
      source = tomlFormat.generate "obayebar-config.toml" settings;
    };

    # Every program the bar starts for the user — an application from the
    # launcher, a browser for a GitLab todo, the lock screen — is put in a
    # transient unit under this slice rather than in the bar's own cgroup, so
    # that restarting the bar does not close them. Declaring the slice is what
    # gives that namespace a description and a memory policy; systemd would
    # otherwise create it implicitly with neither.
    systemd.user.slices.${sliceUnit} = lib.mkIf cfg.systemd.enable {
      Unit = {
        Description = "Programs launched by obayebar";
      };

      Slice = {
        MemoryAccounting = true;
      }
      // lib.optionalAttrs cfg.systemd.managedOom {
        ManagedOOMMemoryPressure = "kill";
        ManagedOOMSwap = "kill";
      };
    };

    systemd.user.services.obayebar = lib.mkIf cfg.systemd.enable {
      Unit = {
        Description = "Obayebar Wayland Status Bar";
        After = [ cfg.systemd.target ];
        PartOf = [ cfg.systemd.target ];
      };

      Service = {
        Type = "exec";
        ExecStart = execStart;
        Restart = "on-failure";
        RestartSec = "5s";
        TimeoutStopSec = "5s";
        Slice = "session.slice";
      };

      Install = {
        WantedBy = [ cfg.systemd.target ];
      };
    };

    # Its own unit rather than something the bar starts. The two are
    # independent — a bar crash should not blank the desktop, and restarting
    # the bar should not make every wallpaper flicker while it redraws.
    systemd.user.services.obayebar-wallpaper =
      lib.mkIf (cfg.wallpaper.enable && cfg.systemd.enable) {
        Unit = {
          Description = "Obayebar wallpaper renderer";
          After = [ cfg.systemd.target ];
          PartOf = [ cfg.systemd.target ];
        };

        Service = {
          Type = "exec";
          ExecStart = "${cfg.package}/bin/obayebar-wallpaper";
          # Reloading re-scans the directory without changing what is on
          # screen, so `systemctl --user reload` picks up new pictures.
          ExecReload = "${cfg.package}/bin/obayebar-wallpaper --reload";
          Restart = "on-failure";
          RestartSec = "5s";
          TimeoutStopSec = "5s";
          Slice = "session.slice";
        };

        Install = {
          WantedBy = [ cfg.systemd.target ];
        };
      };

    # hypridle is what actually notices you have stopped typing; obayebar-lock
    # is only one of the things it runs. Wiring them here keeps the timeouts
    # and the locker from drifting apart in two different config files.
    services.hypridle = lib.mkIf cfg.idle.enable {
      enable = true;
      settings = {
        # No locker configured, no lock commands: hypridle would otherwise
        # run a binary the rest of the module never set up.
        general = lib.optionalAttrs cfg.lock.enable {
          lock_cmd = replacingLockCmd;
          # Lock before the machine suspends, so the screen is never briefly
          # unlocked on resume.
          before_sleep_cmd = "${replacingLockCmd} --detach";
        };
        listener = idleListeners;
      };
    };
  };
}
