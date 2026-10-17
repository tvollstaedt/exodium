#!/bin/sh
# build-gsplus-appimage.sh — Build GSplus (KEGS 1.38 + SDL3) from its release
# tag and package it as an "anylinux" AppImage.
#
# Upstream's Linux tarball links a system SDL3, which most distributions do
# not carry yet; the AppImage bundles it. Runs every eXoAppleIIGS game on
# Linux (§23).
#
# Runs inside ghcr.io/pkgforge-dev/archlinux:latest prepared by
# pkgforge-dev/anylinux-setup-action. Same recipe as
# build-dosbox-x-appimage.sh; read that one first.
#
# Outputs into ./dist:
#   GSplus.AppImage                  stable name - the resolver probes
#                                    content/emulators/gsplus/GSplus.AppImage
#   gsplus-<version>-source.tar.gz   GPL-3.0 source correspondence (the exact
#                                    tag compiled)
set -eu

VERSION="${GSPLUS_VERSION:?GSPLUS_VERSION must be set (e.g. 1.38.0)}"
ARCH="$(uname -m)"

echo "Installing build dependencies..."
pacman -Syu --noconfirm base-devel git cmake ninja sdl3 alsa-lib libpulse libglvnd mesa
get-debloated-pkgs --add-common --prefer-nano

echo "Fetching GSplus ${VERSION} sources..."
mkdir -p ./dist
curl -fL --retry 5 --retry-delay 3 -o "./dist/gsplus-${VERSION}-source.tar.gz" \
    "https://github.com/digarok/gsplus/archive/refs/tags/v${VERSION}.tar.gz"
tar xzf "./dist/gsplus-${VERSION}-source.tar.gz"
cd "gsplus-${VERSION}"

# The system SDL3 is found and used; the AppImage then carries it.
cmake -G Ninja -B build -S gsplus/src -DCMAKE_BUILD_TYPE=Release
cmake --build build --target gsplus-sdl
GSPLUS_BIN="$(pwd)/build/gsplus"
[ -x "$GSPLUS_BIN" ] || { echo "ERROR: build/gsplus was not produced"; exit 1; }
install -Dm755 "$GSPLUS_BIN" /usr/bin/gsplus
cd ..

# quick-sharun wants an icon and a .desktop file to name the AppImage.
ICON=/usr/share/icons/hicolor/256x256/apps/gsplus.png
mkdir -p "$(dirname "$ICON")" /usr/share/applications
cp "gsplus-${VERSION}/gsplus/lib/gsplusicon.png" "$ICON"
DESKTOP=/usr/share/applications/gsplus.desktop
cat > "$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=GSplus
Exec=gsplus
Icon=gsplus
Categories=Game;Emulator;
EOF

export ARCH VERSION ICON DESKTOP
export OUTPATH=./dist
export DEPLOY_OPENGL=1
export DEPLOY_PULSE=1

echo "Packaging with quick-sharun..."
quick-sharun /usr/bin/gsplus
quick-sharun --make-appimage
quick-sharun --test ./dist/*.AppImage

for f in ./dist/*.AppImage; do
    mv "$f" ./dist/GSplus.AppImage
    break
done
rm -f ./dist/*.zsync
chmod +x ./dist/GSplus.AppImage

echo "Done:"
ls -la ./dist
