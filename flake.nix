{
  description = "obayebar - Wayland status bar inspired by caelestia-shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    naersk.url = "github:nix-community/naersk";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, naersk, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        rustNightly = pkgs.rust-bin.selectLatestNightlyWith (toolchain:
          toolchain.default.override {
            extensions = [ "rust-src" "clippy" "rustfmt" "rust-analyzer" "rustc-codegen-cranelift-preview" ];
          }
        );

        naersk' = pkgs.callPackage naersk {
          cargo = rustNightly;
          rustc = rustNightly;
        };

        buildInputs = with pkgs; [
          wayland
          libxkbcommon
          vulkan-loader
          fontconfig
          pipewire
        ];

        nativeBuildInputs = with pkgs; [
          pkg-config
          clang
          llvmPackages.libclang
        ];

        runtimeDeps = with pkgs; [
          material-symbols
        ];
      in {
        packages.default = naersk'.buildPackage {
          src = ./.;
          inherit buildInputs nativeBuildInputs;

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath buildInputs;
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          # Wrap binary to include font paths
          postInstall = ''
            wrapProgram $out/bin/obayebar \
              --set OBAYEBAR_FONT_DIR "${pkgs.material-symbols}/share/fonts/TTF"
          '';
        };

        devShells.default = pkgs.mkShell {
          inherit buildInputs;
          nativeBuildInputs = nativeBuildInputs ++ [
            rustNightly
            pkgs.mold
            pkgs.clang
            pkgs.llvmPackages.libclang
          ];

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath buildInputs;
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          OBAYEBAR_FONT_DIR = "${pkgs.material-symbols}/share/fonts/TTF";
        };
      }
    );
}
