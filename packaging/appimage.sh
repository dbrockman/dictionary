#!/usr/bin/env bash
# Packages a release binary as Dictionary-<version>-<arch>.AppImage, plus
# the .zsync file that AppImage update tools use.
#
#   packaging/appimage.sh <binary> <version> [out-dir]
#
# Run it on an old distribution (CI uses Ubuntu 22.04), since the AppImage
# needs the glibc the binary was linked against or newer. A binary from the
# Nix build won't work: its interpreter and libraries are in /nix/store.
#
# Needs ImageMagick, curl and file. appimagetool is downloaded unless
# APPIMAGETOOL points at it.
#
# The binary links xcb and xkbcommon and dlopens Wayland, Vulkan, fontconfig
# and freetype. All of them come from the host, as for other AppImages: every
# desktop has them, and xkbcommon must match the host's keyboard and compose
# data (an older bundled copy logs errors about newer Compose files).
set -euo pipefail

binary=$1
version=$2
out_dir=${3:-.}
arch=$(uname -m)
here=$(cd "$(dirname "$0")" && pwd)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
appdir=$work/AppDir

install -Dm755 "$binary" "$appdir/usr/bin/dictionary"
install -Dm644 "$here/dictionary.desktop" "$appdir/usr/share/applications/dictionary.desktop"
for size in 16 24 32 48 64 128 256 512; do
  dir=$appdir/usr/share/icons/hicolor/${size}x${size}/apps
  mkdir -p "$dir"
  convert "$here/dictionary.png" -resize "${size}x${size}" -strip "$dir/dictionary.png"
done

# appimagetool wants the desktop file and icon at the root, and AppRun as the
# entry point.
ln -s usr/share/applications/dictionary.desktop "$appdir/dictionary.desktop"
ln -s usr/share/icons/hicolor/256x256/apps/dictionary.png "$appdir/dictionary.png"
ln -s usr/bin/dictionary "$appdir/AppRun"

tool=${APPIMAGETOOL:+$(realpath "$APPIMAGETOOL")}
if [[ -z $tool ]]; then
  tool=$work/appimagetool
  curl -fsSL -o "$tool" \
    "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$arch.AppImage"
  chmod +x "$tool"
fi

mkdir -p "$out_dir"
output=Dictionary-$version-$arch.AppImage
# Lets AppImageUpdate and similar tools find newer releases on GitHub.
# appimagetool writes the .zsync file they need into the current folder.
update_info="gh-releases-zsync|dbrockman|dictionary|latest|Dictionary-*-$arch.AppImage.zsync"
# Extracting avoids needing FUSE, which containers and CI runners lack.
cd "$out_dir"
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=$arch VERSION=$version "$tool" -u "$update_info" "$appdir" "$output"
ls "$PWD/$output"*
