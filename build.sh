#!/usr/bin/env bash
# Builds gitlog. Works in the project container and on the host
# (both Debian trixie/sid).
#
# System dependencies (GTK4 dev libraries) are NOT installed by default;
# pass --install-system-deps to install them via sudo apt-get.
set -euo pipefail
cd "$(dirname "$0")"

INSTALL_SYSTEM_DEPS=false
for arg in "$@"; do
    case "$arg" in
        --install-system-deps)
            INSTALL_SYSTEM_DEPS=true
            ;;
        *)
            echo "Unknown argument: $arg" >&2
            echo "Usage: ./build.sh [--install-system-deps]" >&2
            exit 1
            ;;
    esac
done

# --- system dependencies -----------------------------------------------------
if ! command -v pkg-config >/dev/null 2>&1 || ! pkg-config --exists gtk4 glib-2.0; then
    if [ "$INSTALL_SYSTEM_DEPS" = true ]; then
        echo "Installing GTK4 development libraries..."
        sudo apt-get update
        sudo apt-get install -y pkg-config libgtk-4-dev
    else
        echo "Missing system dependencies: GTK4 development libraries" >&2
        echo "(pkg-config, gtk4, glib-2.0)." >&2
        echo >&2
        echo "Re-run with --install-system-deps to install them via apt (needs sudo):" >&2
        echo "  ./build.sh --install-system-deps" >&2
        exit 1
    fi
fi

# --- rust toolchain ------------------------------------------------------------
if ! command -v cargo >/dev/null 2>&1; then
    if [ -f "$HOME/.cargo/env" ]; then
        # shellcheck disable=SC1091
        . "$HOME/.cargo/env"
    fi
fi
if ! command -v cargo >/dev/null 2>&1; then
    echo "Installing Rust toolchain..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain stable --profile minimal
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
fi

# --- build ---------------------------------------------------------------------
cargo build --release

echo
echo "Done. Run with:"
echo "  target/release/gitlog <path-to-git-repo>     # commit history"
echo "  target/release/gitcommit [path-to-git-repo]  # commit window"
echo "  target/release/gitpush [path-to-git-repo]    # push window"
