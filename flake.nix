{
  description = "A fast native dictionary app built with GPUI";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAllSystems (pkgs:
        let
          # Loaded at runtime by GPUI (windowing, input, GPU, fonts).
          runtimeLibraries = with pkgs; [
            wayland
            libxkbcommon
            vulkan-loader
            libxcb
            libx11
            fontconfig
            freetype
          ];
        in
        {
          default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [ cargo rustc clippy rustfmt rust-analyzer pkg-config just ];
            buildInputs = runtimeLibraries;
            env = {
              RUST_BACKTRACE = "1";
              LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibraries;
            };
          };
        });
    };
}
