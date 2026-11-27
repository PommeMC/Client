{
  description = "Pomme development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      nixpkgs,
      rust-overlay,
      ...
    }:
    let
      inherit (nixpkgs) lib;

      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      forAllSystems =
        f:
        lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ (import rust-overlay) ];
            }
          )
        );
    in
    {
      devShells = forAllSystems (
        pkgs:
        let
          # The channel and components come from rust-toolchain.toml, so the
          # shell builds with the same nightly as rustup and CI.
          toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

          # dlopened at runtime rather than linked: Vulkan (pyronyx `loaded`),
          # winit's Wayland and X11 backends, and OpenAL's system fallback.
          runtimeLibs = with pkgs; [
            vulkan-loader
            wayland
            libxkbcommon
            libx11
            libxcursor
            libxi
            libxrandr
            openal
            libGL
          ];
        in
        {
          default = pkgs.mkShell {
            packages =
              [ toolchain ]
              ++ (with pkgs; [
                just
                python3
                pkg-config
                cmake
                clang
                lld
                git
                jdk25

                shaderc
                udev
                zlib

                vulkan-tools
                vulkan-validation-layers
              ])
              ++ runtimeLibs;

            # shaderc-sys links the nixpkgs build instead of compiling its own.
            SHADERC_LIB_DIR = "${lib.getLib pkgs.shaderc}/lib";
            LIBCLANG_PATH = "${lib.getLib pkgs.llvmPackages.libclang}/lib";
            LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibs;
            VK_LAYER_PATH = "${pkgs.vulkan-validation-layers}/share/vulkan/explicit_layer.d";
          };
        }
      );

      formatter = forAllSystems (pkgs: pkgs.nixfmt-tree);
    };
}
