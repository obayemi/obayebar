self: {
  config,
  pkgs,
  lib,
  ...
}:
let
  inherit (pkgs.stdenv.hostPlatform) system;
  cfg = config.programs.obayebar;
  defaultPackage = self.packages.${system}.default;
in {
  options.programs.obayebar = with lib; {
    enable = mkEnableOption "obayebar Wayland status bar";

    package = mkOption {
      type = types.package;
      default = defaultPackage;
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
  };

  config = lib.mkIf cfg.enable {
    home.packages = [cfg.package];

    systemd.user.services.obayebar = lib.mkIf cfg.systemd.enable {
      Unit = {
        Description = "Obayebar Wayland Status Bar";
        After = [cfg.systemd.target];
        PartOf = [cfg.systemd.target];
      };

      Service = {
        Type = "exec";
        ExecStart = "${cfg.package}/bin/obayebar";
        Restart = "on-failure";
        RestartSec = "5s";
        TimeoutStopSec = "5s";
        Slice = "session.slice";
      };

      Install = {
        WantedBy = [cfg.systemd.target];
      };
    };
  };
}
