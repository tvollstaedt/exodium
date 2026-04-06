#!/usr/bin/env bash
# get-dosbox.sh — Download DOSBox Staging for the current platform into src-tauri/binaries/.
#
# Usage:
#   pnpm run get-dosbox              # download latest supported version
#   VERSION=0.82.0 pnpm run get-dosbox
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARIES_DIR="$REPO_ROOT/src-tauri/binaries"
VERSION="${VERSION:-0.82.0}"

mkdir -p "$BINARIES_DIR"

# ── Detect platform ───────────────────────────────────────────────────────────

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Darwin)
    case "$ARCH" in
      arm64)  TRIPLE="aarch64-apple-darwin";    ASSET_OS="macOS" ;;
      x86_64) TRIPLE="x86_64-apple-darwin";     ASSET_OS="macOS" ;;
      *) echo "Unsupported macOS arch: $ARCH"; exit 1 ;;
    esac
    BIN_NAME="dosbox-staging"
    ARCHIVE_EXT="tar.gz"
    ;;
  Linux)
    case "$ARCH" in
      x86_64)  TRIPLE="x86_64-unknown-linux-gnu";   ASSET_OS="Linux" ;;
      aarch64) TRIPLE="aarch64-unknown-linux-gnu";   ASSET_OS="Linux-aarch64" ;;
      *) echo "Unsupported Linux arch: $ARCH"; exit 1 ;;
    esac
    BIN_NAME="dosbox-staging"
    ARCHIVE_EXT="tar.gz"
    ;;
  MINGW*|MSYS*|CYGWIN*)
    TRIPLE="x86_64-pc-windows-msvc"
    ASSET_OS="Windows"
    BIN_NAME="dosbox-staging.exe"
    ARCHIVE_EXT="zip"
    ;;
  *)
    echo "Unsupported OS: $OS"; exit 1 ;;
esac

OUT_BIN="$BINARIES_DIR/dosbox-staging-$TRIPLE"
[[ "$OS" == MINGW* || "$OS" == MSYS* || "$OS" == CYGWIN* ]] && OUT_BIN="${OUT_BIN}.exe"

if [[ -f "$OUT_BIN" ]]; then
  echo "dosbox-staging-$TRIPLE already present, skipping."
  exit 0
fi

# ── Resolve download URL ──────────────────────────────────────────────────────

BASE_URL="https://github.com/dosbox-staging/dosbox-staging/releases/download/v${VERSION}"

case "$ASSET_OS" in
  macOS)
    # Universal binary covers both Intel and ARM
    ARCHIVE="dosbox-staging-macOS-v${VERSION}.dmg"
    ;;
  Linux)
    ARCHIVE="dosbox-staging-Linux-x86_64-v${VERSION}.tar.gz"
    ;;
  Linux-aarch64)
    ARCHIVE="dosbox-staging-Linux-aarch64-v${VERSION}.tar.gz"
    ;;
  Windows)
    ARCHIVE="dosbox-staging-Windows-v${VERSION}.zip"
    ;;
esac

DOWNLOAD_URL="$BASE_URL/$ARCHIVE"

# ── Download & extract ────────────────────────────────────────────────────────

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "Downloading DOSBox Staging $VERSION for $TRIPLE..."
curl -fL --progress-bar -o "$TMP_DIR/$ARCHIVE" "$DOWNLOAD_URL"

echo "Extracting..."

if [[ "$ARCHIVE" == *.dmg ]]; then
  # macOS DMG — mount, copy binary, unmount
  MOUNT_POINT="$(mktemp -d)"
  hdiutil attach -quiet -nobrowse -mountpoint "$MOUNT_POINT" "$TMP_DIR/$ARCHIVE"
  # Binary is named "dosbox" inside the .app bundle
  cp "$MOUNT_POINT/DOSBox Staging.app/Contents/MacOS/dosbox" "$OUT_BIN"
  hdiutil detach -quiet "$MOUNT_POINT"
  rm -rf "$MOUNT_POINT"
elif [[ "$ARCHIVE" == *.tar.gz ]]; then
  tar -xzf "$TMP_DIR/$ARCHIVE" -C "$TMP_DIR"
  FOUND="$(find "$TMP_DIR" -name "dosbox-staging" -not -name "*.sh" -type f | head -1)"
  cp "$FOUND" "$OUT_BIN"
elif [[ "$ARCHIVE" == *.zip ]]; then
  unzip -q "$TMP_DIR/$ARCHIVE" -d "$TMP_DIR/extracted"
  FOUND="$(find "$TMP_DIR/extracted" -name "dosbox-staging.exe" -type f | head -1)"
  cp "$FOUND" "$OUT_BIN"
fi

chmod +x "$OUT_BIN"
echo "Installed: $OUT_BIN"
