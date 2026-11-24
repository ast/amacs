# The whole amacs workspace as one package: the editor and server (`amacs`),
# the clients (`amacsclient`, `amacs-gui`) and the server for language
# models (`amacs-mcp`), out of one cargo invocation, with the systemd user
# units of the server.
#
# It is a file of its own, to be called with `callPackage`, so that the
# flake's package and its overlay are the same derivation from the same
# file, built with whatever nixpkgs calls it.
{
  lib,
  rustPlatform,
  pkg-config,
  libxkbcommon,
  wayland,
  vulkan-loader,
}:

rustPlatform.buildRustPackage (finalAttrs: {
  pname = "amacs";
  version = "0.1.0";

  # What the build and the tests read, by name. The documentation and the
  # files of the development shell are left out, so that a change to them
  # does not rebuild everything, and so is `target/`, which git hides from
  # a flake but not from `callPackage` on a plain path.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
      ../runtime
      ../examples
      ../contrib
    ];
  };

  # The lock file that is checked in says what is built: there is no hash
  # to keep in step with it, and no `outputHashes`, since every crate in it
  # is from crates.io.
  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [ pkg-config ];

  # The Wayland client reads keymaps with libxkbcommon, and talks to the
  # compositor through libwayland, as the drivers of GPUs do.
  buildInputs = [
    libxkbcommon
    wayland
  ];

  # The tests are a check of the flake, `checks.tests`, built in the dev
  # profile beside this release build rather than after it. They start
  # programs and draw text, which wants more than this build has.
  doCheck = false;

  # User units for running the server under systemd, with or without
  # socket activation.
  postInstall = ''
    install -Dm644 -t $out/lib/systemd/user contrib/systemd/amacs.socket
    substitute contrib/systemd/amacs.service $out/lib/systemd/user/amacs.service \
      --replace-fail @bindir@ $out/bin
  '';

  # Drawing with the GPU loads Vulkan when it is asked for, by name.
  postFixup = ''
    patchelf --add-rpath ${lib.makeLibraryPath [ vulkan-loader ]} $out/bin/amacs-gui
  '';

  # Not wrapped, on purpose. The editor runs the programs of the user: git,
  # ripgrep for searching a project, language servers, formatters, a shell.
  # It finds them on the PATH it is started with, as it finds the ones of a
  # project through direnv, and `amacsclient` knows a server by the name of
  # its process, which a wrapper would change.

  meta = {
    description = "Emacs-like editor with a Rust core, extended in its own Scheme";
    homepage = "https://github.com/ast/amacs";
    # A terminal, Wayland, pseudo-terminals and /proc.
    platforms = lib.platforms.linux;
    mainProgram = "amacs";
  };
})
