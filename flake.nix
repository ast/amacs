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
      # What the build and the tests read. The documentation and the files
      # of the development shell are left out, so that a change to them
      # does not rebuild everything.
      src = nixpkgs.lib.fileset.toSource {
        root = ./.;
        fileset = nixpkgs.lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./crates
          ./runtime
          ./examples
          ./contrib
        ];
      };
    in
    {
      packages = forAllSystems (pkgs: rec {
        amacs = pkgs.rustPlatform.buildRustPackage {
          pname = "amacs";
          version = "0.1.0";
          inherit src;
          cargoLock.lockFile = ./Cargo.lock;
          # The Wayland client reads keymaps with libxkbcommon, and talks to
          # the compositor through libwayland, as the drivers of GPUs do.
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [
            pkgs.libxkbcommon
            pkgs.wayland
          ];
          # The tests are a check of their own, `checks.tests`, built in the
          # dev profile beside this release build rather than after it.
          doCheck = false;
          # Drawing with the GPU loads Vulkan when it is asked for, by name.
          postFixup = ''
            patchelf --add-rpath ${pkgs.lib.makeLibraryPath [ pkgs.vulkan-loader ]} $out/bin/amacs-gui
          '';
          # User units for running the server under systemd, with or without
          # socket activation.
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

          # The whole test suite, in the dev profile that `just check` runs.
          tests = amacs.overrideAttrs (old: {
            pname = "amacs-tests";
            # Searching a project's files runs ripgrep, and so do its tests.
            nativeBuildInputs = old.nativeBuildInputs ++ [
              pkgs.ripgrep
              pkgs.git
            ];
            # The renderer's tests draw text and need a known font.
            AMACS_TEST_FONTS = "${pkgs.jetbrains-mono}/share/fonts";
            # Some tests expand `~`.
            buildPhase = "HOME=$TMPDIR cargo test --workspace --offline -j $NIX_BUILD_CORES";
            doCheck = false;
            installPhase = "touch $out";
            # Nothing is installed, so there is nothing to fix up.
            dontFixup = true;
          });

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
          # Link with mold. An environment variable of this shell rather than
          # a `.cargo/config.toml`: the Nix build reads that file too, and
          # has no mold. `mold` is mold behind the same wrapper as
          # the toolchain's ld, which adds the paths of the store libraries.
          "CARGO_TARGET_${pkgs.stdenv.hostPlatform.rust.cargoEnvVarTarget}_RUSTFLAGS" =
            "-C link-arg=-fuse-ld=mold";
          packages = with pkgs; [
            mold
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
