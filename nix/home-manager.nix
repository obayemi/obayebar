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

  # The union of every section, not just GitLab's. Gating on one feature's
  # attrs meant a wallpaper-only configuration produced no config.toml at all,
  # with no warning — the file simply was not written.
  settings =
    lib.optionalAttrs (gitlabAttrs != { }) { gitlab = gitlabAttrs; }
    // lib.optionalAttrs (wallpaperAttrs != { }) { wallpaper = wallpaperAttrs; }
    // lib.optionalAttrs (lockAttrs != { }) { lock = lockAttrs; };

  hasConfig = settings != { };

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
        example = "/home/you/Images/wallpapers/enabled";
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
        example = "/home/you/.config/hypr/hyprlock.conf";
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

      idle = {
        enable = mkEnableOption "hypridle, locking the session after a timeout";

        timeout = mkOption {
          type = types.int;
          default = 300;
          description = "Seconds of inactivity before the screen locks.";
        };
      };
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    xdg.configFile."obayebar/config.toml" = lib.mkIf hasConfig {
      source = tomlFormat.generate "obayebar-config.toml" settings;
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
    # is only the thing it runs. Wiring both here keeps the timeout and the
    # locker from drifting apart in two different config files.
    services.hypridle = lib.mkIf (cfg.lock.enable && cfg.lock.idle.enable) {
      enable = true;
      settings = {
        general = {
          lock_cmd = "${cfg.package}/bin/obayebar-lock";
          # Lock before the machine suspends, so the screen is never briefly
          # unlocked on resume.
          before_sleep_cmd = "${cfg.package}/bin/obayebar-lock --detach";
        };
        listener = [
          {
            inherit (cfg.lock.idle) timeout;
            on-timeout = "${cfg.package}/bin/obayebar-lock";
          }
        ];
      };
    };
  };
}
