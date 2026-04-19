{
  lib,
  naersk,
  makeWrapper,
  pkg-config,
  clang,
  llvmPackages,
  wayland,
  libxkbcommon,
  vulkan-loader,
  fontconfig,
  pipewire,
  material-symbols,
}:
let
  buildInputs = [
    wayland
    libxkbcommon
    vulkan-loader
    fontconfig
    pipewire
  ];

  nativeBuildInputs = [
    pkg-config
    clang
    llvmPackages.libclang
    makeWrapper
  ];
in
naersk.buildPackage {
  pname = "obayebar";
  src = ./..;
  inherit buildInputs nativeBuildInputs;

  LD_LIBRARY_PATH = lib.makeLibraryPath buildInputs;
  LIBCLANG_PATH = "${llvmPackages.libclang.lib}/lib";

  postInstall = ''
    wrapProgram $out/bin/obayebar \
      --set OBAYEBAR_FONT_DIR "${material-symbols}/share/fonts/TTF" \
      --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath buildInputs}"
  '';

  meta = {
    description = "Wayland status bar inspired by caelestia-shell";
    homepage = "https://github.com/obayemi/obayebar";
    license = lib.licenses.mit;
    mainProgram = "obayebar";
  };
}
