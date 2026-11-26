{
  description = "amacs: an Emacs-like editor with a Rust core and Scheme extensions";

  # What this repository is built and checked with. A flake that takes
  # this one as an input sets `inputs.nixpkgs.follows`, and then this is not
  # in its lock; the package needs Rust 1.95 or later of the nixpkgs it is
  # built with.
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
      packages = forAllSystems (
        pkgs:
        let
          amacs = pkgs.callPackage ./nix/package.nix { };
        in
        {
          inherit amacs;
          default = amacs;
        }
      );

      # For a NixOS or home-manager configuration: with this among
      # `nixpkgs.overlays`, `pkgs.amacs` is the editor built with the
      # nixpkgs of that configuration, whatever this flake is locked to.
      overlays.default = final: _prev: {
        amacs = final.callPackage ./nix/package.nix { };
      };

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
            # The SFTP client is tested against the sftp-server of OpenSSH.
            nativeBuildInputs = old.nativeBuildInputs ++ [
              pkgs.ripgrep
              pkgs.git
              pkgs.openssh
              # The file manager packs and unpacks with these, besides the
              # tar, gzip, xz and bzip2 that every build has.
              pkgs.zstd
              pkgs.zip
              pkgs.unzip
              # What is encrypted is encrypted by these.
              pkgs.gnupg
              pkgs.age
            ];
            # zip and unzip are run in a UTF-8 locale, for names that are not
            # ASCII, and a build has no locales unless it is given them.
            LOCALE_ARCHIVE = "${pkgs.glibcLocales}/lib/locale/locale-archive";
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
            zstd
            zip
            unzip
            gnupg
            age
          ];
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
