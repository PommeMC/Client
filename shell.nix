{
  # Pinned so every machine builds against the same packages; bump the rev
  # deliberately, as with the toolchain in rust-toolchain.toml.
  pkgs ? import (fetchTarball "https://github.com/NixOS/nixpkgs/archive/d4f25a3e562008648d45bcf2d70bcc7422cad84e.tar.gz") {},
}:
let
  inherit (pkgs) lib;
in
pkgs.mkShell {
  packages = with pkgs; [
    cargo-nextest
    cmake
    jdk25
    just
    pkg-config
    python3
    rustup
  ];

  buildInputs = with pkgs; [
    dbus
    udev
    vulkan-loader
  ];

  SHADERC_LIB_DIR = "${lib.getLib pkgs.shaderc}/lib";

  # Libraries the client opens at runtime: the window system, the Vulkan
  # loader, and the audio backends the staged OpenAL Soft looks for.
  LD_LIBRARY_PATH = lib.makeLibraryPath (with pkgs; [
    alsa-lib
    libGL
    libpulseaudio
    libx11
    libxcursor
    libxi
    libxkbcommon
    libxrandr
    udev
    vulkan-loader
    wayland
  ]);
}
