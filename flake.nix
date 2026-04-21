{
  description = "obayebar - Wayland status bar inspired by caelestia-shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    naersk.url = "github:nix-community/naersk";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, naersk, flake-utils, rust-overlay }:
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
          naersk' = pkgs.callPackage naersk {
            cargo = rustNightly;
            rustc = rustNightly;
          };
          deps = buildInputs pkgs;
        in
        naersk'.buildPackage {
          pname = "obayebar";
          inherit src;
          buildInputs = deps;
          nativeBuildInputs = nativeBuildInputs pkgs;

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
          };

          config = lib.mkIf cfg.enable {
            home.packages = [ cfg.package ];

            systemd.user.services.obayebar = lib.mkIf cfg.systemd.enable {
              Unit = {
                Description = "Obayebar Wayland Status Bar";
                After = [ cfg.systemd.target ];
                PartOf = [ cfg.systemd.target ];
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
