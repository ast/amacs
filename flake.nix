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
          # The check phase runs the whole test suite. Some tests expand `~`.
          preCheck = "export HOME=$TMPDIR";
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
          });

          fmt = amacs.overrideAttrs (old: {
            pname = "amacs-fmt";
            nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.rustfmt ];
            buildPhase = "cargo fmt --all --check";
            doCheck = false;
            installPhase = "touch $out";
          });
        }
      );

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.amacs ];
          packages = with pkgs; [
            clippy
            rustfmt
            rust-analyzer
            cargo-deny
            cargo-insta
          ];
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
