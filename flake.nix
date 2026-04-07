{
  description = "obayebar - Wayland status bar inspired by caelestia-shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    naersk.url = "github:nix-community/naersk";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, naersk, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        naersk' = pkgs.callPackage naersk { };

        buildInputs = with pkgs; [
          wayland
          libxkbcommon
          vulkan-loader
          fontconfig
        ];

        nativeBuildInputs = with pkgs; [
          pkg-config
        ];

        runtimeDeps = with pkgs; [
          material-symbols
        ];
      in {
        packages.default = naersk'.buildPackage {
          src = ./.;
          inherit buildInputs nativeBuildInputs;

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath buildInputs;

          # Wrap binary to include font paths
          postInstall = ''
            wrapProgram $out/bin/obayebar \
              --set OBAYEBAR_FONT_DIR "${pkgs.material-symbols}/share/fonts/TTF"
          '';
        };

        devShells.default = pkgs.mkShell {
          inherit buildInputs;
          nativeBuildInputs = nativeBuildInputs ++ (with pkgs; [
            rustc
            cargo
            clippy
            rustfmt
            mold
            clang
          ]);

          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath buildInputs;
          OBAYEBAR_FONT_DIR = "${pkgs.material-symbols}/share/fonts/TTF";
        };
      }
    );
}
