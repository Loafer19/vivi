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

          vivi = pkgs.rustPlatform.buildRustPackage {
            inherit pname version;

            src = self;

            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = with pkgs; [
              pkg-config
            ];

            buildInputs = with pkgs; [
              # Media engine
              mpv

              # Database
              sqlite

              # Slint / Winit / OpenGL
              fontconfig
              libxkbcommon
              wayland
              libglvnd

              # X11
              libx11
              libxcursor
              libxi
              libxrandr
              libxinerama
            ];

            # Nix's cargo setup hook exports this as 1 unconditionally, which
            # overrides rusqlite's `bundled` feature. Override it before Cargo builds.
            preBuild = ''
              export LIBSQLITE3_SYS_USE_PKG_CONFIG=0
            '';

            # Build metadata
            VIVI_GIT_HASH =
              if self ? shortRev then
                self.shortRev
              else if self ? dirtyShortRev then
                self.dirtyShortRev
              else
                "unknown";

            postInstall = ''
              # Desktop entry
              install -Dm644 \
                packaging/vivi.desktop \
                $out/share/applications/vivi.desktop

              # Application icon
              install -Dm644 \
                packaging/vivi.svg \
                $out/share/icons/hicolor/scalable/apps/vivi.svg

              # License
              install -Dm644 \
                LICENSE \
                $out/share/licenses/${pname}/LICENSE
            '';

            meta = {
              description = "Fast and lightweight media player";
              homepage = "https://github.com/Loafer19/vivi";
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
    };
}
