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

  hasConfig = gitlabAttrs != { };

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
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    xdg.configFile."obayebar/config.toml" = lib.mkIf hasConfig {
      source = tomlFormat.generate "obayebar-config.toml" { gitlab = gitlabAttrs; };
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
  };
}
