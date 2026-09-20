#!/usr/bin/env bash
# Installs the project binaries to ~/.local/bin and the nemo context menu
# actions to ~/.local/share/nemo/actions (Nemo is NOT installed/started here).
# Arguments are passed to build.sh, e.g. --install-system-deps.
set -euo pipefail
cd "$(dirname "$0")"

BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
NEMO_ACTIONS_DIR="${NEMO_ACTIONS_DIR:-$HOME/.local/share/nemo/actions}"

# --- build -------------------------------------------------------------------
# Passes arguments through to build.sh (e.g. --install-system-deps).
./build.sh "$@"

# --- install binaries --------------------------------------------------------
mkdir -p "$BIN_DIR"
install -m 0755 target/release/gitlog    "$BIN_DIR/gitlog"
install -m 0755 target/release/gitcommit "$BIN_DIR/gitcommit"

# --- install nemo actions ----------------------------------------------------
mkdir -p "$NEMO_ACTIONS_DIR"
install -m 0644 res/gitlog.nemo_action    "$NEMO_ACTIONS_DIR/gitlog.nemo_action"
install -m 0644 res/gitcommit.nemo_action "$NEMO_ACTIONS_DIR/gitcommit.nemo_action"
install -m 0755 res/check-git-dir.sh      "$NEMO_ACTIONS_DIR/check-git-dir.sh"

echo
echo "Installed:"
echo "  $BIN_DIR/gitlog"
echo "  $BIN_DIR/gitcommit"
echo "  $NEMO_ACTIONS_DIR/gitlog.nemo_action"
echo "  $NEMO_ACTIONS_DIR/gitcommit.nemo_action"
echo "  $NEMO_ACTIONS_DIR/check-git-dir.sh"
echo
echo "Make sure $BIN_DIR is in your PATH, then restart Nemo"
echo "(e.g. log out and in) to pick up the context menu actions."
