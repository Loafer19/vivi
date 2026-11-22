{
  description = "Vivi - Rust media player with Slint and libmpv";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs, ... }:

    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      forAllSystems = nixpkgs.lib.genAttrs systems;

      # Libraries Slint (winit + femtovg) and libmpv load at runtime via dlopen.
      runtimeLibs =
        pkgs: with pkgs; [
          libGL
          wayland
          libxkbcommon
          fontconfig
          vulkan-loader
          libx11
          libxcursor
          libxi
          libxrandr
          libxinerama
        ];

    in
    {
      packages = forAllSystems (
        system:

        let
          pkgs = import nixpkgs {
            inherit system;
          };

          lib = pkgs.lib;

          cargoToml = lib.importTOML ./Cargo.toml;

          pname = cargoToml.package.name;
          version = cargoToml.package.version;

          libs = runtimeLibs pkgs;

          vivi = pkgs.rustPlatform.buildRustPackage {
            inherit pname version;

            src = lib.fileset.toSource {
              root = ./.;
              fileset = lib.fileset.unions [
                ./Cargo.toml
                ./Cargo.lock
                ./build.rs
                ./LICENSE
                ./src
                ./ui
                ./assets
                ./packaging/vivi.desktop
                ./packaging/vivi.svg
                ./packaging/vivi.png
                ./packaging/flatpak/io.github.loafer19.vivi.metainfo.xml
              ];
            };

            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = with pkgs; [
              pkg-config
              makeWrapper
            ];

            buildInputs = [ pkgs.mpv-unwrapped ] ++ libs;

            # Nix's cargo setup hook exports this as 1 unconditionally, which
            # overrides rusqlite's `bundled` feature. Override it before Cargo builds.
            preBuild = ''
              export LIBSQLITE3_SYS_USE_PKG_CONFIG=0
            '';

            # Match the from-source AUR package (`options=('!lto')`): the
            # release profile's fat LTO is too slow and memory-heavy for Nix.
            env.CARGO_PROFILE_RELEASE_LTO = "off";

            doCheck = false;

            # Build metadata
            VIVI_GIT_HASH =
              if self ? shortRev then
                self.shortRev
              else if self ? dirtyShortRev then
                self.dirtyShortRev
              else
                "unknown";

            postInstall = ''
              install -Dm644 \
                packaging/vivi.desktop \
                $out/share/applications/vivi.desktop

              install -Dm644 \
                packaging/vivi.svg \
                $out/share/icons/hicolor/scalable/apps/vivi.svg
              install -Dm644 \
                packaging/vivi.png \
                $out/share/icons/hicolor/256x256/apps/vivi.png

              # Same launchable rewrite as packaging/aur/vivi-git/PKGBUILD.
              sed 's|<launchable type="desktop-id">.*</launchable>|<launchable type="desktop-id">vivi.desktop</launchable>|' \
                packaging/flatpak/io.github.loafer19.vivi.metainfo.xml \
                > metainfo.xml
              install -Dm644 metainfo.xml \
                $out/share/metainfo/io.github.loafer19.vivi.metainfo.xml

              install -Dm644 \
                LICENSE \
                $out/share/licenses/${pname}/LICENSE
            '';

            # yt-dlp and ffmpeg are optional and are not added to PATH here.
            # A system ffmpeg on PATH is used for library thumbnails and album
            # covers; the YouTube plugin can install a managed yt-dlp (and a
            # small ffmpeg on demand for download merge/convert); Chapter
            # Thumbnails uses ffmpeg for scene detection. Local playback needs
            # neither. Wrapping them would also shadow the plugin's managed copies.
            postFixup = ''
              wrapProgram $out/bin/vivi \
                --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath libs}
            '';

            meta = {
              description = "Fast and lightweight media player";
              homepage = "https://viviplayer.app";
              license = lib.licenses.mit;
              platforms = lib.platforms.linux;
              mainProgram = "vivi";
            };
          };

        in
        {
          default = vivi;
          vivi = vivi;
        }
      );

      # Allow: nix run .#vivi
      apps = forAllSystems (
        system:
        let
          pkg = self.packages.${system}.vivi;

        in
        {
          default = {
            type = "app";
            program = "${pkg}/bin/vivi";
          };

          vivi = {
            type = "app";
            program = "${pkg}/bin/vivi";
          };

          vivi-mcp = {
            type = "app";
            program = "${pkg}/bin/vivi-mcp";
          };
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
          };
          lib = pkgs.lib;
          libs = runtimeLibs pkgs;
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rustc
              cargo
              pkg-config
            ];
            buildInputs = [ pkgs.mpv-unwrapped ] ++ libs;
            LD_LIBRARY_PATH = lib.makeLibraryPath libs;
            # Same rusqlite workaround as the package, so `cargo build` in the
            # shell does not pick up Nix's pkg-config sqlite.
            shellHook = ''
              export LIBSQLITE3_SYS_USE_PKG_CONFIG=0
            '';
          };
        }
      );
    };
}
