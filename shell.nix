{
  pkgs ? import <nixpkgs> {},
}:
let
  inherit (pkgs) lib;
in
pkgs.mkShell {
  packages = with pkgs; [
    cargo-nextest
    just
    pkg-config
    rustup
  ];

  buildInputs = with pkgs; [
    dbus
    udev
    vulkan-loader
  ];

  VULKAN_SDK = "${pkgs.vulkan-headers}";
  SHADERC_LIB_DIR = "${lib.getLib pkgs.shaderc}/lib";

  LD_LIBRARY_PATH = lib.makeLibraryPath (with pkgs; [
    libGL
    libx11
    libxcursor
    libxi
    libxkbcommon
    libxrandr
    udev
    vulkan-loader
  ]);
}
