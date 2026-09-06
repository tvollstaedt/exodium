#!/usr/bin/env bash
# build-emulator-packs.sh — Assemble the emulator content-pack tarballs for one
# platform. The packs replace the emulators that used to ship inside the
# installer (344 MB of the macOS bundle served 29-of-662 games).
#
#   --emulator win9x (default)
#   --platform macos   dosbox-x-macos-arm64-v<N>.tar.gz   (upstream .app, arm64)
#                      86box-macos-universal-v<N>.tar.gz  (upstream .app)
#   --platform linux   dosbox-x-linux-x86_64-v<N>.tar.gz  (our self-built
#                        AppImage, pass it via --dosbox-x-appimage <path>)
#                      86box-linux-x86_64-v<N>.tar.gz     (upstream AppImage)
#
#   --emulator scummvm --scummvm-version <v>
#   --platform macos   scummvm-<v>-macos-universal-v<N>.tar.gz (upstream DMG)
#   --platform linux   scummvm-<v>-linux-x86_64-v<N>.tar.gz    (our self-built
#                        AppImage, pass it via --scummvm-appimage <path>)
#
# Tarball rules (each one guards a shipped incident - see CLAUDE.md §10/§16):
#   - payload wrapped in a dir named EXACTLY like the pack id ("dosbox-x/",
#     "86box/") so unwrapped_source strips it by name
#   - COPYFILE_DISABLE=1 on macOS: AppleDouble ._* sidecars inside a .app sit
#     next to _CodeSignature and can break the bundle
#   - .apps are xattr-cleared and ad-hoc signed BEFORE tarring: arm64 macOS
#     SIGKILLs an invalid signature, and the runtime repair is debug-only
#   - exec bits set before tarring (tar preserves them, the installer doesn't fix them)
#
# Usage:
#   scripts/build-emulator-packs.sh --platform macos
#   scripts/build-emulator-packs.sh --platform linux --dosbox-x-appimage dist/DOSBox-X.AppImage
#   DOSBOX_X_VERSION=2025.02.01 E86BOX_VERSION=6.0 PACK_VERSION=1 ...
set -euo pipefail

PLATFORM=""
EMULATOR="win9x"
DBX_APPIMAGE=""
SVM_APPIMAGE=""
SCUMMVM_VERSION=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --platform) PLATFORM="$2"; shift 2 ;;
    --emulator) EMULATOR="$2"; shift 2 ;;
    --dosbox-x-appimage) DBX_APPIMAGE="$2"; shift 2 ;;
    --scummvm-appimage) SVM_APPIMAGE="$2"; shift 2 ;;
    --scummvm-version) SCUMMVM_VERSION="$2"; shift 2 ;;
    *) echo "Unknown argument: $1"; exit 1 ;;
  esac
done
[[ -z "$PLATFORM" ]] && { echo "Usage: $0 --platform macos|linux [--emulator win9x|scummvm]"; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="$REPO_ROOT/dist/packs"
mkdir -p "$OUT_DIR"

DOSBOX_X_VERSION="${DOSBOX_X_VERSION:-2025.02.01}"
E86BOX_VERSION="${E86BOX_VERSION:-6.0}"
PACK_VERSION="${PACK_VERSION:-1}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

fetch() { # fetch <url> <out>
  echo "Downloading $(basename "$2")..."
  curl -fL --progress-bar --retry 5 --retry-delay 3 --retry-all-errors -C - -o "$2" "$1"
}

gh_api() { # gh_api <url> — token when set; CI runners share the anon allowance
  if [[ -n "${GITHUB_TOKEN:-}" ]]; then
    curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" "$1"
  else
    curl -fsSL "$1"
  fi
}

asset_url() { # asset_url <repo> <tag> <grep-pattern> — names embed build stamps
  gh_api "https://api.github.com/repos/$1/releases/tags/$2" \
    | grep -o 'https://[^"]*download/[^"]*' | grep -E "$3" | head -1
}

# License texts ship inside each pack (both emulators are GPLv2; the source
# tarballs go onto the same content release).
fetch_license() { # fetch_license <raw-url> <out>
  curl -fsSL -o "$2" "$1" || echo "WARNING: could not fetch license $1"
}

roll_tar() { # roll_tar <staging-parent> <wrapper-name> <out.tar.gz>
  # COPYFILE_DISABLE keeps bsdtar from writing ._* AppleDouble sidecars; on
  # GNU tar it is simply an ignored env var.
  (cd "$1" && COPYFILE_DISABLE=1 tar czf "$3" "$2")
  echo "Packed: $3"
}

if [[ "$EMULATOR" == "scummvm" ]]; then
  [[ -z "$SCUMMVM_VERSION" ]] && { echo "ERROR: --scummvm-version <v> is required"; exit 1; }
  # The wrapper repeats install_path's last segment (content/emulators/
  # scummvm-<v>), which is what unwrapped_source strips by name.
  STAGE="$TMP_DIR/stage-svm/scummvm-${SCUMMVM_VERSION}"
  mkdir -p "$STAGE"

  case "$PLATFORM" in
    macos)
      [[ "$(uname -s)" == "Darwin" ]] || { echo "--platform macos must run on macOS (codesign)"; exit 1; }
      # The universal DMG serves both Mac architectures - verified on 2.5.0:
      # Contents/MacOS/scummvm is x86_64 + arm64.
      fetch "https://downloads.scummvm.org/frs/scummvm/${SCUMMVM_VERSION}/scummvm-${SCUMMVM_VERSION}-macosx.dmg" \
        "$TMP_DIR/svm.dmg"
      MNT="$TMP_DIR/mnt-svm"
      mkdir -p "$MNT"
      hdiutil attach -nobrowse -quiet "$TMP_DIR/svm.dmg" -mountpoint "$MNT"
      trap 'hdiutil detach "'"$MNT"'" -quiet 2>/dev/null || true; rm -rf "$TMP_DIR"' EXIT
      [[ -d "$MNT/ScummVM.app" ]] || { echo "ERROR: ScummVM.app not in the DMG"; exit 1; }
      cp -R "$MNT/ScummVM.app" "$STAGE/ScummVM.app"
      cp "$MNT/COPYRIGHT" "$STAGE/COPYRIGHT" 2>/dev/null || true
      cp "$MNT/License (GPL)" "$STAGE/COPYING" 2>/dev/null || true
      hdiutil detach "$MNT" -quiet
      trap 'rm -rf "$TMP_DIR"' EXIT

      # Sparkle cannot be stripped (scummvm links it via @rpath), so the
      # self-updater is disabled instead - same reason the DOSBox-X AppImage
      # ships without its updater hook: a build that replaces itself under
      # eXo's pin is exactly what the pin exists to prevent. "Check for
      # Updates" in the menu still works; nothing happens on its own.
      plutil -replace SUEnableAutomaticChecks -bool false "$STAGE/ScummVM.app/Contents/Info.plist"
      plutil -replace SUAutomaticallyUpdate -bool false "$STAGE/ScummVM.app/Contents/Info.plist"

      xattr -cr "$STAGE/ScummVM.app" 2>/dev/null || true
      codesign --force --deep --sign - "$STAGE/ScummVM.app"
      chmod +x "$STAGE/ScummVM.app/Contents/MacOS/scummvm"
      roll_tar "$TMP_DIR/stage-svm" "scummvm-${SCUMMVM_VERSION}" \
        "$OUT_DIR/scummvm-${SCUMMVM_VERSION}-macos-universal-v${PACK_VERSION}.tar.gz"
      ;;

    linux)
      [[ -z "$SVM_APPIMAGE" ]] && { echo "ERROR: --scummvm-appimage <path> is required for linux"; exit 1; }
      [[ -f "$SVM_APPIMAGE" ]] || { echo "ERROR: $SVM_APPIMAGE not found"; exit 1; }
      cp "$SVM_APPIMAGE" "$STAGE/ScummVM.AppImage"
      chmod +x "$STAGE/ScummVM.AppImage"
      fetch_license "https://raw.githubusercontent.com/scummvm/scummvm/v${SCUMMVM_VERSION}/COPYING" "$STAGE/COPYING"
      roll_tar "$TMP_DIR/stage-svm" "scummvm-${SCUMMVM_VERSION}" \
        "$OUT_DIR/scummvm-${SCUMMVM_VERSION}-linux-x86_64-v${PACK_VERSION}.tar.gz"
      ;;

    *)
      echo "Unknown platform: $PLATFORM (expected macos or linux)"; exit 1 ;;
  esac

else

case "$PLATFORM" in
  macos)
    [[ "$(uname -s)" == "Darwin" ]] || { echo "--platform macos must run on macOS (codesign)"; exit 1; }

    # ── dosbox-x (upstream arm64 .app; the pinned version) ──────────────────
    STAGE="$TMP_DIR/stage-dbx/dosbox-x"
    mkdir -p "$STAGE"
    URL="$(asset_url joncampbell123/dosbox-x "dosbox-x-v${DOSBOX_X_VERSION}" 'macosx-arm64-[^"]*\.zip')"
    [[ -z "$URL" ]] && { echo "ERROR: no DOSBox-X macOS arm64 asset for v${DOSBOX_X_VERSION}"; exit 1; }
    fetch "$URL" "$TMP_DIR/dbx-mac.zip"
    unzip -q "$TMP_DIR/dbx-mac.zip" -d "$TMP_DIR/dbx"
    APP_SRC="$(find "$TMP_DIR/dbx" -type d -name "dosbox-x.app" | head -1)"
    [[ -z "$APP_SRC" ]] && { echo "ERROR: dosbox-x.app not found in the archive"; exit 1; }
    cp -R "$APP_SRC" "$STAGE/dosbox-x.app"
    xattr -cr "$STAGE/dosbox-x.app" 2>/dev/null || true
    codesign --force --deep --sign - "$STAGE/dosbox-x.app"
    chmod +x "$STAGE/dosbox-x.app/Contents/MacOS/dosbox-x"
    fetch_license "https://raw.githubusercontent.com/joncampbell123/dosbox-x/dosbox-x-v${DOSBOX_X_VERSION}/COPYING" "$STAGE/COPYING"
    roll_tar "$TMP_DIR/stage-dbx" "dosbox-x" "$OUT_DIR/dosbox-x-macos-arm64-v${PACK_VERSION}.tar.gz"

    # ── 86box (upstream universal .app) ─────────────────────────────────────
    STAGE="$TMP_DIR/stage-86b/86box"
    mkdir -p "$STAGE"
    URL="$(asset_url 86Box/86Box "v${E86BOX_VERSION}" '86Box-macOS-[^"]*\.zip')"
    [[ -z "$URL" ]] && { echo "ERROR: no 86Box macOS asset for v${E86BOX_VERSION}"; exit 1; }
    fetch "$URL" "$TMP_DIR/86b-mac.zip"
    unzip -q "$TMP_DIR/86b-mac.zip" -d "$TMP_DIR/e86"
    APP_SRC="$(find "$TMP_DIR/e86" -type d -name "86Box.app" | head -1)"
    [[ -z "$APP_SRC" ]] && { echo "ERROR: 86Box.app not found in the archive"; exit 1; }
    cp -R "$APP_SRC" "$STAGE/86Box.app"
    xattr -cr "$STAGE/86Box.app" 2>/dev/null || true
    codesign --force --deep --sign - "$STAGE/86Box.app"
    chmod +x "$STAGE/86Box.app/Contents/MacOS/86Box"
    fetch_license "https://raw.githubusercontent.com/86Box/86Box/v${E86BOX_VERSION}/COPYING" "$STAGE/COPYING"
    roll_tar "$TMP_DIR/stage-86b" "86box" "$OUT_DIR/86box-macos-universal-v${PACK_VERSION}.tar.gz"
    ;;

  linux)
    # ── dosbox-x (our self-built AppImage from build-dosbox-x-appimage.sh) ──
    [[ -z "$DBX_APPIMAGE" ]] && { echo "ERROR: --dosbox-x-appimage <path> is required for linux"; exit 1; }
    [[ -f "$DBX_APPIMAGE" ]] || { echo "ERROR: $DBX_APPIMAGE not found"; exit 1; }
    STAGE="$TMP_DIR/stage-dbx/dosbox-x"
    mkdir -p "$STAGE"
    cp "$DBX_APPIMAGE" "$STAGE/DOSBox-X.AppImage"
    chmod +x "$STAGE/DOSBox-X.AppImage"
    fetch_license "https://raw.githubusercontent.com/joncampbell123/dosbox-x/dosbox-x-v${DOSBOX_X_VERSION}/COPYING" "$STAGE/COPYING"
    roll_tar "$TMP_DIR/stage-dbx" "dosbox-x" "$OUT_DIR/dosbox-x-linux-x86_64-v${PACK_VERSION}.tar.gz"

    # ── 86box (upstream AppImage, same asset get-emulators.sh bundles today) ─
    STAGE="$TMP_DIR/stage-86b/86box"
    mkdir -p "$STAGE"
    URL="$(asset_url 86Box/86Box "v${E86BOX_VERSION}" '86Box-Linux-x86_64[^"]*\.AppImage')"
    [[ -z "$URL" ]] && { echo "ERROR: no 86Box Linux asset for v${E86BOX_VERSION}"; exit 1; }
    fetch "$URL" "$STAGE/86Box.AppImage"
    chmod +x "$STAGE/86Box.AppImage"
    fetch_license "https://raw.githubusercontent.com/86Box/86Box/v${E86BOX_VERSION}/COPYING" "$STAGE/COPYING"
    roll_tar "$TMP_DIR/stage-86b" "86box" "$OUT_DIR/86box-linux-x86_64-v${PACK_VERSION}.tar.gz"
    ;;

  *)
    echo "Unknown platform: $PLATFORM (expected macos or linux)"; exit 1 ;;
esac

fi

echo
echo "sha256 / sizes for manifest.json:"
for f in "$OUT_DIR"/*.tar.gz; do
  if command -v sha256sum >/dev/null 2>&1; then
    HASH="$(sha256sum "$f" | cut -d' ' -f1)"
  else
    HASH="$(shasum -a 256 "$f" | cut -d' ' -f1)"
  fi
  SIZE="$(wc -c < "$f" | tr -d ' ')"
  echo "  $(basename "$f")  sha256=$HASH  size_bytes=$SIZE"
done
