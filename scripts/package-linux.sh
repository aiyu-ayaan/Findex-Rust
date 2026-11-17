#!/usr/bin/env bash
# Builds the Linux release files from a compiled binary:
#   dist/Findex-<version>-x86_64.AppImage   (logo inside: .DirIcon, hicolor PNGs 16-512 and SVG)
#   dist/findex_<version>_amd64.deb
#
#   scripts/package-linux.sh [path/to/findex]     default: target/release/findex
#
# Needs: rsvg-convert (librsvg2-bin) or resvg for the PNG icons, dpkg-deb, and appimagetool
# (APPIMAGETOOL=path, or it is downloaded once into target/).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
bin="${1:-target/release/findex}"
[ -x "$bin" ] || { echo "error: no binary at $bin (cargo build --release -p findex-app)" >&2; exit 1; }
version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)"
svg=assets/brand/app-icon.svg
dist=dist
rm -rf "$dist" target/AppDir target/debroot
mkdir -p "$dist"

desktop_entry() { # $1 = Exec line
  cat <<ENTRY
[Desktop Entry]
Type=Application
Name=Findex
GenericName=File Manager
Comment=Fast file explorer with the Windows 11 look
Exec=$1 %U
Icon=findex
Terminal=false
Categories=System;FileTools;FileManager;Utility;
MimeType=inode/directory;
Keywords=files;folders;explorer;manager;
StartupWMClass=findex
ENTRY
}

# SVG to PNG: rsvg-convert (librsvg2-bin), or resvg when that is what is installed.
render() { # $1 = size, $2 = output
  if command -v rsvg-convert >/dev/null; then rsvg-convert -w "$1" -h "$1" "$svg" -o "$2"
  else resvg -w "$1" -h "$1" "$svg" "$2"; fi
}

# Icons: hicolor PNGs at every common size plus the scalable SVG.
install_icons() { # $1 = prefix (…/usr)
  for size in 16 24 32 48 64 128 256 512; do
    d="$1/share/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$d"
    render "$size" "$d/findex.png"
  done
  mkdir -p "$1/share/icons/hicolor/scalable/apps"
  cp "$svg" "$1/share/icons/hicolor/scalable/apps/findex.svg"
}

# ---- AppImage ----
app=target/AppDir
mkdir -p "$app/usr/bin" "$app/usr/share/applications"
install -m755 "$bin" "$app/usr/bin/findex"
install_icons "$app/usr"
desktop_entry findex > "$app/usr/share/applications/findex.desktop"
cp "$app/usr/share/applications/findex.desktop" "$app/findex.desktop"
cp "$app/usr/share/icons/hicolor/256x256/apps/findex.png" "$app/findex.png"
ln -sf findex.png "$app/.DirIcon"
cat > "$app/AppRun" <<'RUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/findex" "$@"
RUN
chmod 755 "$app/AppRun"

tool="${APPIMAGETOOL:-target/appimagetool}"
if [ ! -x "$tool" ]; then
  curl -fsSL --retry 3 -o "$tool" https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
  chmod 755 "$tool"
fi
# --appimage-extract-and-run: works without FUSE (CI runners, containers).
ARCH=x86_64 "$tool" --appimage-extract-and-run --no-appstream "$app" "$dist/Findex-$version-x86_64.AppImage"

# ---- .deb ----
deb=target/debroot
debver="${version/-/\~}" # 0.0.1-alpha.1 -> 0.0.1~alpha.1 so pre-releases sort before the final
mkdir -p "$deb/DEBIAN" "$deb/usr/bin" "$deb/usr/share/applications"
install -m755 "$bin" "$deb/usr/bin/findex"
install_icons "$deb/usr"
desktop_entry /usr/bin/findex > "$deb/usr/share/applications/findex.desktop"
cat > "$deb/DEBIAN/control" <<CONTROL
Package: findex
Version: $debver
Section: utils
Priority: optional
Architecture: amd64
Depends: libc6 (>= 2.31), libfontconfig1, libfreetype6, libxkbcommon0, libgl1
Maintainer: aiyu-ayaan <noreply@github.com>
Homepage: https://github.com/aiyu-ayaan/Findex-Rust
Description: Fast file explorer with the Windows 11 look
 Tabs, every Explorer view mode, fuzzy search, thumbnails and one transfer
 window for copy, move and delete. Can be set as the default file manager.
CONTROL
dpkg-deb --root-owner-group --build "$deb" "$dist/findex_${debver}_amd64.deb" >/dev/null

ls -la "$dist"
