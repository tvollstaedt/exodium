#!/bin/sh
# build-scummvm-appimage.sh — Build one PINNED ScummVM release from source and
# package it as an "anylinux" AppImage.
#
# eXo pins seven ScummVM builds because games break across releases (§18), and
# upstream publishes no Linux binary of any of them - only .deb, Flatpak and a
# source tarball. So Linux gets the pinned version the same way it gets
# DOSBox-X: compiled here, packaged with quick-sharun, named for the resolver.
#
# Runs inside ghcr.io/pkgforge-dev/archlinux:latest prepared by
# pkgforge-dev/anylinux-setup-action. Same recipe as
# build-dosbox-x-appimage.sh; read that one first.
#
# Outputs into ./dist:
#   ScummVM.AppImage                    stable name - resolve_scummvm probes
#                                       content/emulators/scummvm-<v>/ScummVM.AppImage
#   scummvm-<version>-source.tar.xz     GPL source correspondence (upstream's
#                                       own tarball, the exact tree compiled)
set -eu

VERSION="${SCUMMVM_VERSION:?SCUMMVM_VERSION must be set (e.g. 2.9.0)}"
ARCH="$(uname -m)"
BASE="https://downloads.scummvm.org/frs/scummvm/${VERSION}"

echo "Installing build dependencies..."
# ScummVM's configure auto-detects; a missing optional library silently drops
# an engine's audio or video codec, so the list is explicit rather than
# minimal. curl/sdl2_net are what the cloud and multiplayer features need.
pacman -Syu --noconfirm \
    base-devel git nasm \
    sdl2 sdl2_net libpng libjpeg-turbo libtheora libvorbis flac libmad \
    faad2 libmpeg2 a52dec fluidsynth freetype2 zlib curl giflib \
    alsa-lib libglvnd mesa
get-debloated-pkgs --add-common --prefer-nano

echo "Fetching ScummVM ${VERSION} sources..."
mkdir -p ./dist
curl -fL --retry 5 --retry-delay 3 -o "./dist/scummvm-${VERSION}-source.tar.xz" \
    "${BASE}/scummvm-${VERSION}.tar.xz"
tar xf "./dist/scummvm-${VERSION}-source.tar.xz"
cd "scummvm-${VERSION}"

# --enable-release strips the assert-heavy debug build; --prefix=/usr puts the
# engine data, themes and .desktop where the packaging step looks for them.
# The pinned sources predate GCC 15's stricter checks: 2.8.0's bundled
# freetype (ags) fails on unsigned char* -> char*, 2.5.0's icb engine on
# -Wtemplate-body. Both are warnings in the compiler that built them.
export CXXFLAGS="${CXXFLAGS:-} -fpermissive -Wno-template-body"
echo "Building ScummVM ${VERSION}..."
./configure --prefix=/usr --enable-release --enable-all-engines
make -j"$(nproc)"
make install
cd ..

SCUMMVM_BIN="$(command -v scummvm)"
ICON="$(ls /usr/share/icons/hicolor/scalable/apps/scummvm.svg \
          /usr/share/pixmaps/scummvm.xpm 2>/dev/null | head -1)"
DESKTOP="$(ls /usr/share/applications/*scummvm*.desktop \
              /usr/share/applications/org.scummvm.scummvm.desktop 2>/dev/null | head -1)"

export ARCH VERSION ICON DESKTOP
export OUTPATH=./dist
export DEPLOY_OPENGL=1
export DEPLOY_PULSE=1

echo "Packaging with quick-sharun..."
quick-sharun "$SCUMMVM_BIN" /usr/lib/libfluidsynth.so*

# ScummVM loads its GUI theme and per-engine data files at RUNTIME from
# <bin>/../share/scummvm. quick-sharun bundles libraries, not data, so the
# tree is copied in by hand - without it the launcher comes up unthemed and
# several engines refuse to start ("Engine data file missing").
mkdir -p ./AppDir/usr/share
cp -r /usr/share/scummvm ./AppDir/usr/share/
if [ ! -f ./AppDir/usr/share/scummvm/scummmodern.zip ]; then
    echo "ERROR: the GUI theme is missing from the AppDir - the data copy failed."
    exit 1
fi

quick-sharun --make-appimage
quick-sharun --test ./dist/*.AppImage

for f in ./dist/*.AppImage; do
    mv "$f" ./dist/ScummVM.AppImage
    break
done
rm -f ./dist/*.zsync
chmod +x ./dist/ScummVM.AppImage

echo "Done:"
ls -la ./dist
