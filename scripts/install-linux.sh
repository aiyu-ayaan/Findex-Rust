#!/bin/sh
# Install, update or remove Findex on Linux (x86_64), for this user, no root.
#
#   curl -fsSL https://raw.githubusercontent.com/aiyu-ayaan/Findex-Rust/master/scripts/install-linux.sh | sh
#                                     newest stable build; if there is none, the newest beta, then alpha
#   ... | sh -s -- --channel beta     beta builds (and stable ones)
#   ... | sh -s -- --version 0.0.1    a specific release
#   ... | sh -s -- --uninstall        remove it (settings stay in ~/.config/findex)
#   ... | sh -s -- --set-default      also make Findex the default file manager (Super+E on GNOME)
#
# The AppImage goes to ~/.local/share/findex, a place this user owns. It gets a menu entry with
# the app's own logo (taken out of the AppImage, every hicolor size) and a `findex` command.
# Re-running it updates.
#
# POSIX sh: it is piped into whatever `sh` is, which on Debian and Ubuntu is dash.
set -eu

API="${FINDEX_RELEASES_URL:-https://api.github.com/repos/aiyu-ayaan/Findex-Rust/releases?per_page=100}"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
APP="$DATA_HOME/findex/Findex.AppImage"
BIN="$HOME/.local/bin/findex"
# Named like the entry inside the AppImage; StartupWMClass below is what ties the window to it.
DESKTOP_FILE="$DATA_HOME/applications/findex.desktop"
ICON_SIZES="16 24 32 48 64 128 256 512"
ICON_DIR="$DATA_HOME/icons/hicolor"

CHANNEL=""  # empty: take the steadiest channel that has a build
VERSION=""
FROM_FILE=""
UNINSTALL=0
SET_DEFAULT=0

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --channel) CHANNEL="${2:?--channel needs a value}"; shift 2 ;;
    --version) VERSION="${2:?--version needs a value}"; VERSION="${VERSION#v}"; shift 2 ;;
    --from-file) FROM_FILE="${2:?--from-file needs a path}"; shift 2 ;;
    --uninstall) UNINSTALL=1; shift ;;
    --set-default) SET_DEFAULT=1; shift ;;
    -h | --help) sed -n '2,8p' "$0" 2>/dev/null || true; echo "  --from-file PATH   install a local AppImage"; exit 0 ;;
    *) die "unknown option: $1" ;;
  esac
done
case "$CHANNEL" in "" | stable | beta | alpha) ;; *) die "--channel must be stable, beta or alpha" ;; esac
[ "$(id -u)" -ne 0 ] || die "run this as the user who will use Findex, not as root."

refresh_menus() {
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q "$DATA_HOME/applications" 2>/dev/null || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$DATA_HOME/icons/hicolor" 2>/dev/null || true
}

if [ "$UNINSTALL" -eq 1 ]; then
  # Give folders back to the previous file manager first (revert before delete).
  if [ -x "$APP" ]; then "$APP" --repair >/dev/null 2>&1 || true; fi
  rm -f "$APP" "$APP.part" "$DESKTOP_FILE"
  for s in $ICON_SIZES; do rm -f "$ICON_DIR/${s}x${s}/apps/findex.png"; done
  rm -f "$ICON_DIR/scalable/apps/findex.svg"
  # Only our own link.
  if [ -L "$BIN" ] && [ "$(readlink "$BIN")" = "$APP" ]; then rm -f "$BIN"; fi
  rmdir "$DATA_HOME/findex" 2>/dev/null || true
  refresh_menus
  echo "Findex is removed. Its settings are kept in ${XDG_CONFIG_HOME:-$HOME/.config}/findex."
  exit 0
fi

case "$(uname -m)" in x86_64 | amd64) ;; *) die "Findex for Linux is built for x86_64 only; this machine is $(uname -m)." ;; esac

mkdir -p "$DATA_HOME/findex"
trap 'rm -f "$APP.part"' EXIT INT TERM

if [ -n "$FROM_FILE" ]; then
  cp "$FROM_FILE" "$APP.part" || die "cannot read $FROM_FILE"
  label="$(basename "$FROM_FILE")"
else
  if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL --retry 3 "$1"; }
    fetch_to() { curl -fL --retry 3 --progress-bar -o "$2" "$1"; }
  elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO- "$1"; }
    fetch_to() { wget -q --show-progress -O "$2" "$1"; }
  else
    die "curl or wget is needed to download Findex."
  fi

  echo "Looking up Findex releases..."
  releases="$(fetch "$API")" || die "could not reach GitHub. Try again in a while."
  # Every AppImage URL as "<sort key> <version> <url>"; the newest is last after a sort. The key
  # orders by number, then alpha < beta < stable, then the pre-release number (plain `sort -V`
  # puts 0.0.1 before 0.0.1-alpha.1). A channel takes its own builds and every steadier one.
  appimages="$(printf '%s\n' "$releases" | grep -oE 'https://[^"]*/Findex-[0-9][^"/]*-x86_64\.AppImage' | sort -u)"
  find_newest() {
    printf '%s\n' "$appimages" | awk -v channel="$1" -v want="$VERSION" '{
      v = $0; sub(/.*\/Findex-/, "", v); sub(/-x86_64\.AppImage$/, "", v)
      if (v !~ /^[0-9]+\.[0-9]+\.[0-9]+(-(alpha|beta)\.[0-9]+)?$/) next
      split(v, p, /[.-]/)
      stage = 2; n = 99999
      if (p[4] == "alpha") { stage = 0; n = p[5] } else if (p[4] == "beta") { stage = 1; n = p[5] }
      if (want != "" ? v != want : (stage == 0 && channel != "alpha") || (stage == 1 && channel == "stable")) next
      printf "%08d.%08d.%08d.%d.%08d %s %s\n", p[1], p[2], p[3], stage, n, v, $0
    }' | sort | tail -n 1
  }
  if [ -n "$CHANNEL" ] || [ -n "$VERSION" ]; then
    newest="$(find_newest "${CHANNEL:-alpha}")"
  else
    # No channel asked for: stable if there is one, else beta, else alpha.
    newest=""
    for c in stable beta alpha; do
      newest="$(find_newest "$c")"
      [ -z "$newest" ] || { CHANNEL="$c"; break; }
    done
  fi
  [ -n "$newest" ] || die "no Linux build found for ${VERSION:-any channel}. See the releases page."
  label="$(printf '%s' "$newest" | cut -d' ' -f2)"
  echo "Downloading Findex $label..."
  fetch_to "$(printf '%s' "$newest" | cut -d' ' -f3)" "$APP.part" || die "the download failed. Nothing was changed."
fi

# An error page saved under the right name must not become the app.
[ "$(head -c 4 "$APP.part" | od -An -c | tr -d ' \n')" = '177ELF' ] || die "that is not an AppImage. Nothing was changed."
chmod 755 "$APP.part"
# A rename within one directory: a running copy keeps running from the old file until restarted.
mv -f "$APP.part" "$APP"
trap - EXIT INT TERM

# The logo comes out of the AppImage itself, so it is always this build's: every size, plus SVG.
tmp="$(mktemp -d)"
if (cd "$tmp" && "$APP" --appimage-extract 'usr/share/icons/*' >/dev/null 2>&1) && [ -d "$tmp/squashfs-root/usr/share/icons/hicolor" ]; then
  mkdir -p "$ICON_DIR"
  cp -R "$tmp/squashfs-root/usr/share/icons/hicolor/." "$ICON_DIR/"
else
  echo "warning: could not read the logo out of the AppImage; the menu entry will use a generic icon." >&2
fi
rm -rf "$tmp"

# Same fields as the entry inside the AppImage. The icon is an absolute path so it shows without
# waiting for the icon theme cache. MimeType lets Findex be picked as the folder handler.
mkdir -p "$(dirname "$DESKTOP_FILE")"
cat >"$DESKTOP_FILE" <<ENTRY
[Desktop Entry]
Type=Application
Name=Findex
GenericName=File Manager
Comment=Fast file explorer with the Windows 11 look
Exec="$APP" %U
TryExec=$APP
Icon=$ICON_DIR/256x256/apps/findex.png
Terminal=false
Categories=System;FileTools;FileManager;Utility;
MimeType=inode/directory;
StartupWMClass=findex
ENTRY
refresh_menus

mkdir -p "$(dirname "$BIN")"
if [ ! -e "$BIN" ] || [ -L "$BIN" ]; then ln -sf "$APP" "$BIN"; fi

echo "Findex $label is installed: find it in your applications menu, or run 'findex'."
if [ "$SET_DEFAULT" -eq 1 ]; then
  "$APP" --set-default || echo "warning: could not make Findex the default file manager." >&2
else
  echo "To open folders (and Super+E on GNOME) with Findex: findex --set-default   (undo: findex --repair)"
fi
if [ ! -e /dev/fuse ]; then
  echo "warning: FUSE is not available, so Findex unpacks itself on every start (slower). Install fuse3 with your package manager." >&2
fi
