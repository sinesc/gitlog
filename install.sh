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
install -m 0755 target/release/gitpush   "$BIN_DIR/gitpush"

# --- install nemo actions ----------------------------------------------------
# The Comment field of the action files contains a placeholder that is
# replaced with the translation for the system locale. Locale detection
# mirrors src/i18n.rs: the first set variable of LC_ALL / LC_MESSAGES / LANG
# decides, and an unsupported language falls back to English.
raw_locale=""
if [ -n "${LC_ALL+x}" ]; then raw_locale="$LC_ALL"
elif [ -n "${LC_MESSAGES+x}" ]; then raw_locale="$LC_MESSAGES"
elif [ -n "${LANG+x}" ]; then raw_locale="$LANG"
fi
lang="${raw_locale%%[._-]*}"
lang="${lang,,}"
case "$lang" in
  de) gitlog_comment="Zeigt das Git Log für den Ordner";          gitcommit_comment="Öffnet das Git Commit-Fenster für den Ordner"; gitpush_comment="Öffnet das Git Push-Fenster für den Ordner" ;;
  fr) gitlog_comment="Affiche l'historique git du dossier";      gitcommit_comment="Ouvre la fenêtre de commit git du dossier";        gitpush_comment="Ouvre la fenêtre de push git du dossier" ;;
  es) gitlog_comment="Muestra el registro git de la carpeta";    gitcommit_comment="Abre la ventana de commit de git de la carpeta"; gitpush_comment="Abre la ventana de push de git de la carpeta" ;;
  it) gitlog_comment="Mostra la cronologia git della cartella";  gitcommit_comment="Apre la finestra di commit git della cartella";  gitpush_comment="Apre la finestra di push git della cartella" ;;
  nl) gitlog_comment="Toont de git-log van de map";              gitcommit_comment="Opent het git-commitvenster van de map";        gitpush_comment="Opent het git-pushvenster van de map" ;;
  pt) gitlog_comment="Mostra o log do git da pasta";             gitcommit_comment="Abre a janela de commit do git da pasta";       gitpush_comment="Abre a janela de push do git da pasta" ;;
  *)  gitlog_comment="Shows the git log for the folder";         gitcommit_comment="Opens the git commit window for the folder";    gitpush_comment="Opens the git push window for the folder" ;;
esac

install_action() { # $1 = source file, $2 = comment translation
  local tmp
  tmp="$(mktemp)"
  sed "s|@NEMO_ACTION_COMMENT@|$2|" "$1" > "$tmp"
  install -m 0644 "$tmp" "$NEMO_ACTIONS_DIR/$(basename "$1")"
  rm -f "$tmp"
}
mkdir -p "$NEMO_ACTIONS_DIR"
install_action res/gitlog.nemo_action    "$gitlog_comment"
install_action res/gitcommit.nemo_action "$gitcommit_comment"
install_action res/gitpush.nemo_action   "$gitpush_comment"
install -m 0755 res/check-git-dir.sh     "$NEMO_ACTIONS_DIR/check-git-dir.sh"

echo
echo "Installed:"
echo "  $BIN_DIR/gitlog"
echo "  $BIN_DIR/gitcommit"
echo "  $BIN_DIR/gitpush"
echo "  $NEMO_ACTIONS_DIR/gitlog.nemo_action"
echo "  $NEMO_ACTIONS_DIR/gitcommit.nemo_action"
echo "  $NEMO_ACTIONS_DIR/gitpush.nemo_action"
echo "  $NEMO_ACTIONS_DIR/check-git-dir.sh"
echo
echo "Make sure $BIN_DIR is in your PATH, then restart Nemo"
echo "(e.g. log out and in) to pick up the context menu actions."
