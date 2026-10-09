{
  description = "A fast native dictionary app built with GPUI";

  # Prebuilt binaries, pushed by CI for each commit on main. Nix asks before
  # using them; see the README for adding the cache to a NixOS configuration.
  nixConfig = {
    extra-substituters = [ "https://dbrockman.cachix.org" ];
    extra-trusted-public-keys = [ "dbrockman.cachix.org-1:oUXtudDZ0/g0qNzHvdMJSf93Orf8Fn+9NUJMUQJXP9I=" ];
  };

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # Loaded at runtime by GPUI (windowing, input, GPU, fonts).
      runtimeLibraries = pkgs: with pkgs; [
        wayland
        libxkbcommon
        vulkan-loader
        libxcb
        libx11
        fontconfig
        freetype
      ];

      package = pkgs:
        let
          inherit (pkgs) lib;
          cargoToml = lib.importTOML ./Cargo.toml;
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "dictionary";
          inherit (cargoToml.workspace.package) version;

          # Only what Cargo reads, so edits to docs or CI don't change the
          # derivation and miss the binary cache.
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./crates ];
          };
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.pkg-config pkgs.imagemagick ];
          buildInputs = runtimeLibraries pkgs;

          # GPUI dlopens most of these, which an installed binary can only find
          # through its RPATH (the dev shell uses LD_LIBRARY_PATH instead).
          postFixup = ''
            patchelf --add-rpath ${lib.makeLibraryPath (runtimeLibraries pkgs)} $out/bin/dictionary
          '';

          # The window's app_id is "dictionary", so the desktop file must be
          # named after it for Wayland compositors to match the two. The icon
          # goes into the hicolor theme at its standard sizes.
          postInstall = ''
            install -Dm644 ${./packaging/dictionary.desktop} $out/share/applications/dictionary.desktop
            for size in 16 24 32 48 64 128 256 512; do
              dir=$out/share/icons/hicolor/''${size}x''${size}/apps
              mkdir -p $dir
              magick ${./packaging/dictionary.png} -resize ''${size}x''${size} -strip $dir/dictionary.png
            done
          '';

          meta = {
            description = "A fast native dictionary app that reads Apple dictionaries";
            homepage = "https://github.com/dbrockman/dictionary";
            license = lib.licenses.mit;
            platforms = systems;
            mainProgram = "dictionary";
          };
        };
    in
    {
      packages = forAllSystems (pkgs: {
        default = package pkgs;
      });

      overlays.default = final: _prev: {
        dictionary = package final;
      };

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [ cargo rustc clippy rustfmt rust-analyzer pkg-config just ];
          buildInputs = runtimeLibraries pkgs;
          env = {
            RUST_BACKTRACE = "1";
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (runtimeLibraries pkgs);
          };
        };
      });
    };
}
