#!/usr/bin/env bash
# Builds gitlog. Works in the project container and on the host
# (both Debian trixie/sid). Requires sudo for the first system-dep install.
set -euo pipefail
cd "$(dirname "$0")"

# --- system dependencies -----------------------------------------------------
if ! pkg-config --exists gtk4 glib-2.0; then
    echo "Installing GTK4 development libraries..."
    sudo apt-get update
    sudo apt-get install -y pkg-config libgtk-4-dev
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
