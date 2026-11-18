{
  description = "amacs: an Emacs-like editor with a Rust core and Scheme extensions";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        amacs = pkgs.rustPlatform.buildRustPackage {
          pname = "amacs";
          version = "0.1.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          # The Wayland client reads keymaps with libxkbcommon, and talks to
          # the compositor through libwayland, as the drivers of GPUs do.
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [
            pkgs.libxkbcommon
            pkgs.wayland
          ];
          # Searching a project's files runs ripgrep, and so do its tests.
          nativeCheckInputs = [
            pkgs.ripgrep
            pkgs.git
          ];
          # The renderer's tests draw text and need a known font.
          AMACS_TEST_FONTS = "${pkgs.jetbrains-mono}/share/fonts";
          # The check phase runs the whole test suite. Some tests expand `~`.
          preCheck = "export HOME=$TMPDIR";
          # User units for running the server under systemd, with or without
          # socket activation.
          # Drawing with the GPU loads Vulkan when it is asked for, by name.
          postFixup = ''
            patchelf --add-rpath ${pkgs.lib.makeLibraryPath [ pkgs.vulkan-loader ]} $out/bin/amacs-gui
          '';
          postInstall = ''
            install -Dm644 -t $out/lib/systemd/user contrib/systemd/amacs.socket
            substitute contrib/systemd/amacs.service $out/lib/systemd/user/amacs.service \
              --replace-fail @bindir@ $out/bin
          '';
          meta.mainProgram = "amacs";
        };
        default = amacs;
      });

      # `nix flake check` is the CI entry point: build, tests, lints, formatting.
      checks = forAllSystems (
        pkgs:
        let
          inherit (self.packages.${pkgs.stdenv.hostPlatform.system}) amacs;
        in
        {
          inherit amacs;

          clippy = amacs.overrideAttrs (old: {
            pname = "amacs-clippy";
            nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
            buildPhase = "cargo clippy --workspace --all-targets --offline -- -D warnings";
            doCheck = false;
            installPhase = "touch $out";
            # Nothing is installed, so there is nothing to fix up.
            dontFixup = true;
          });

          fmt = amacs.overrideAttrs (old: {
            pname = "amacs-fmt";
            nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.rustfmt ];
            buildPhase = "cargo fmt --all --check";
            doCheck = false;
            installPhase = "touch $out";
            # Nothing is installed, so there is nothing to fix up.
            dontFixup = true;
          });
        }
      );

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.amacs ];
          AMACS_TEST_FONTS = "${pkgs.jetbrains-mono}/share/fonts";
          # What is built here finds Vulkan the way the installed program
          # does through its rpath.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [ pkgs.vulkan-loader ];
          packages = with pkgs; [
            clippy
            rustfmt
            rust-analyzer
            cargo-deny
            just
            cargo-insta
            ripgrep
          ];
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
