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
            ./crates
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

          # Every dependency now resolves from crates.io, so no outputHashes
          # are needed (iced_layershell 0.19.1 ships the disable_clipboard
          # opt-out that previously forced a git pin).
          cargoLock.lockFile = ./Cargo.lock;

          buildInputs = deps;
          nativeBuildInputs = nativeBuildInputs pkgs;

          # Tests aren't free to run during the package build (no display,
          # no dbus); the dev workflow uses `cargo test` directly.
          doCheck = false;

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath deps;
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          # Only the bar draws with iced/wgpu, so only it needs the icon font;
          # the wallpaper renderer and the lock screen talk to the compositor
          # through libwayland.
          postInstall = ''
            wrapProgram $out/bin/obayebar \
              --set OBAYEBAR_FONT_DIR "${pkgs.material-symbols}/share/fonts" \
              --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath deps}"
            wrapProgram $out/bin/obayebar-wallpaper \
              --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath deps}"
            wrapProgram $out/bin/obayebar-lock \
              --set OBAYEBAR_HYPRLOCK "${pkgs.hyprlock}/bin/hyprlock" \
              --prefix PATH : "${pkgs.lib.makeBinPath [ pkgs.systemd ]}" \
              --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath [ pkgs.wayland ]}"
          '';

          meta = {
            description = "Wayland status bar inspired by caelestia-shell";
            homepage = "https://github.com/obayemi/obayebar";
            # obayebar itself is MIT; obayebar-lock, built into the same
            # derivation, links wayland-protocols-hyprland, which is
            # LGPL-3.0-only.
            license = with pkgs.lib.licenses; [ mit lgpl3Only ];
            mainProgram = "obayebar";
          };
        };
    in
    {
      overlays.default = final: _prev: {
        obayebar = self.packages.${final.stdenv.hostPlatform.system}.default;
      };

      homeManagerModules.default = import ./nix/home-manager.nix { inherit self; };
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
          OBAYEBAR_FONT_DIR = "${pkgs.material-symbols}/share/fonts";
        };
      }
    );
}
