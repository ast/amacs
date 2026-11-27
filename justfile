# Run `just` to list the recipes.

# The socket of the server these recipes start and stop, kept apart from
# one that may be running for real work. Same as in .envrc.
export AMACS_SOCKET := env("AMACS_SOCKET", env("XDG_RUNTIME_DIR", "/tmp") / "amacs/dev.sock")

# "dev" or "release". The dev loop builds dev: incremental, with optimised
# dependencies, and shared with `cargo test`. Same as in .envrc and .mcp.json.
profile := env("AMACS_PROFILE", "dev")
bindir := "target" / (if profile == "dev" { "debug" } else { profile })

log := "target/amacs-daemon.log"

default:
    @just --list

# Build, stop the server that is running and start a new one.
restart: build stop start

# Build the binaries of the chosen profile.
build:
    cargo build --profile {{profile}}

# Stop the server, if one is running.
stop:
    #!/usr/bin/env bash
    set -u
    {{bindir}}/amacsclient --kill-server >/dev/null 2>&1 || true
    # A new server cannot take the socket until the old one has let go.
    for _ in $(seq 50); do
        [ -S "$AMACS_SOCKET" ] || exit 0
        sleep 0.1
    done
    echo "the old server still holds $AMACS_SOCKET" >&2
    exit 1

# Start a server in the background. What it prints goes to a log.
start:
    #!/usr/bin/env bash
    set -eu
    mkdir -p "$(dirname "$AMACS_SOCKET")"
    # The Scheme that is part of the editor is read from this checkout,
    # so that a restart picks up changes to it without a build.
    # It is set up by the example init file, unless AMACS_INIT names
    # another: the same as in .envrc.
    AMACS_RUNTIME="$PWD/runtime" AMACS_INIT="${AMACS_INIT:-$PWD/examples/init.scm}" \
        setsid nohup {{bindir}}/amacs --daemon >"{{log}}" 2>&1 </dev/null &
    for _ in $(seq 50); do
        if [ -S "$AMACS_SOCKET" ]; then
            echo "amacs is running on $AMACS_SOCKET (log: {{log}})"
            exit 0
        fi
        sleep 0.1
    done
    echo "the server did not start; see {{log}}:" >&2
    cat "{{log}}" >&2
    exit 1

# Open a window on the server, with any files given.
open *files:
    {{bindir}}/amacsclient -c -n {{files}}

# What the commit routine checks, short of the flake.
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    cargo deny check

# The same checks on evo, from a copy of this working tree, committed or not.
check-remote host="evo":
    rsync -a --delete --exclude target --exclude .direnv ./ {{host}}:src/amacs-dev/
    ssh {{host}} 'cd src/amacs-dev && nix develop -c just check'
