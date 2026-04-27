{
  description = "obayebar - Wayland status bar inspired by caelestia-shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    let
      buildInputs = pkgs: with pkgs; [
        wayland
        libxkbcommon
        vulkan-loader
        fontconfig
        pipewire
      ];

      nativeBuildInputs = pkgs: with pkgs; [
        pkg-config
        clang
        llvmPackages.libclang
        mold
        makeWrapper
      ];

      src = let fs = nixpkgs.lib.fileset; in
        fs.toSource {
          root = ./.;
          fileset = fs.unions [
            ./Cargo.toml
            ./Cargo.lock
            ./.cargo
            ./src
          ];
        };

      mkPackage = pkgs:
        let
          rustNightly = pkgs.rust-bin.selectLatestNightlyWith (toolchain:
            toolchain.default
          );
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustNightly;
            rustc = rustNightly;
          };
          deps = buildInputs pkgs;
        in
        rustPlatform.buildRustPackage {
          pname = "obayebar";
          version = "0.1.0";
          inherit src;

          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              # Pinned via [patch.crates-io] in Cargo.toml. All crates come
              # from the same exwlshelleventloop tree, so they share one hash.
              "iced_exdevtools-0.18.0-beta4" = "sha256-CA0KAv8FggttW6kbn4SzJDWsgYxzoWGeIqTUDNBrdFI=";
              "iced_layershell-0.18.0-beta4" = "sha256-CA0KAv8FggttW6kbn4SzJDWsgYxzoWGeIqTUDNBrdFI=";
              "iced_layershell_macros-0.18.0-beta4" = "sha256-CA0KAv8FggttW6kbn4SzJDWsgYxzoWGeIqTUDNBrdFI=";
              "layershellev-0.18.0-beta4" = "sha256-CA0KAv8FggttW6kbn4SzJDWsgYxzoWGeIqTUDNBrdFI=";
              "waycrate_xkbkeycode-0.18.0-beta4" = "sha256-CA0KAv8FggttW6kbn4SzJDWsgYxzoWGeIqTUDNBrdFI=";
            };
          };

          buildInputs = deps;
          nativeBuildInputs = nativeBuildInputs pkgs;

          # Tests aren't free to run during the package build (no display,
          # no dbus); the dev workflow uses `cargo test` directly.
          doCheck = false;

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath deps;
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          postInstall = ''
            wrapProgram $out/bin/obayebar \
              --set OBAYEBAR_FONT_DIR "${pkgs.material-symbols}/share/fonts/TTF" \
              --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath deps}"
            wrapProgram $out/bin/obayebar-launcher \
              --set OBAYEBAR_FONT_DIR "${pkgs.material-symbols}/share/fonts/TTF" \
              --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath deps}"
          '';

          meta = {
            description = "Wayland status bar inspired by caelestia-shell";
            homepage = "https://github.com/obayemi/obayebar";
            license = pkgs.lib.licenses.mit;
            mainProgram = "obayebar";
          };
        };
    in
    {
      overlays.default = final: _prev: {
        obayebar = self.packages.${final.stdenv.hostPlatform.system}.default;
      };

      homeManagerModules.default = { config, pkgs, lib, ... }:
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
        };
    }
    // flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        deps = buildInputs pkgs;
      in {
        packages.default = mkPackage pkgs;

        devShells.default = pkgs.mkShell {
          buildInputs = deps;
          nativeBuildInputs = [
            (pkgs.rust-bin.selectLatestNightlyWith (toolchain:
              toolchain.default.override {
                extensions = [ "rust-src" "clippy" "rustfmt" "rust-analyzer" "rustc-codegen-cranelift-preview" ];
              }
            ))
            pkgs.pkg-config
            pkgs.clang
            pkgs.llvmPackages.libclang
            pkgs.mold
          ];

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath deps;
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          OBAYEBAR_FONT_DIR = "${pkgs.material-symbols}/share/fonts/TTF";
        };
      }
    );
}
