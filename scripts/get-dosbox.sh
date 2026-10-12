#!/usr/bin/env bash
# get-dosbox.sh — Download DOSBox Staging for the current platform into src-tauri/binaries/.
#
# Usage:
#   pnpm run get-dosbox              # download latest supported version
#   pnpm run get-dosbox -- --force   # re-download even if binary already exists
#   VERSION=0.82.0 pnpm run get-dosbox
set -euo pipefail

FORCE=0
for arg in "$@"; do
  [[ "$arg" == "--force" ]] && FORCE=1
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARIES_DIR="$REPO_ROOT/src-tauri/binaries"
VERSION="${VERSION:-0.83.0}"

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
    ;;
  Linux)
    case "$ARCH" in
      x86_64)  TRIPLE="x86_64-unknown-linux-gnu";   ASSET_OS="Linux" ;;
      aarch64) TRIPLE="aarch64-unknown-linux-gnu";   ASSET_OS="Linux-aarch64" ;;
      *) echo "Unsupported Linux arch: $ARCH"; exit 1 ;;
    esac
    BIN_NAME="dosbox-staging"
    ;;
  MINGW*|MSYS*|CYGWIN*)
    TRIPLE="x86_64-pc-windows-msvc"
    ASSET_OS="Windows"
    BIN_NAME="dosbox-staging.exe"
    ;;
  *)
    echo "Unsupported OS: $OS"; exit 1 ;;
esac

OUT_BIN="$BINARIES_DIR/dosbox-staging-$TRIPLE"
[[ "$OS" == MINGW* || "$OS" == MSYS* || "$OS" == CYGWIN* ]] && OUT_BIN="${OUT_BIN}.exe"

# Tauri bundles these from here via bundle.resources. Must exist before
# `tauri build` runs, otherwise the build script errors with
# "resource path doesn't exist". Declared early so the outer skip-check
# can require both binary AND staged resources before bailing out.
STAGED_SHADERS="$REPO_ROOT/src-tauri/resources/dosbox-shaders"
STAGED_PRESETS="$REPO_ROOT/src-tauri/resources/dosbox-shader-presets"
# Keyboard layouts and the code-page fonts they need: without BOTH, Staging
# silently runs every game on the US layout.
STAGED_KEYBOARD="$REPO_ROOT/src-tauri/resources/dosbox-freedos-keyboard"
STAGED_CPI="$REPO_ROOT/src-tauri/resources/dosbox-freedos-cpi"

# Same trick for the dosbox-bin folder. On Windows we populate it below
# with the entire DOSBox Staging folder (dosbox-staging.exe + 9 DLLs +
# codepage resources). On macOS/Linux we leave a placeholder so Tauri's
# resource path validation passes and nothing extra ends up in the bundle.
STAGED_DBS_BIN="$REPO_ROOT/src-tauri/resources/dosbox-bin"
if [[ ! -d "$STAGED_DBS_BIN" ]]; then
  mkdir -p "$STAGED_DBS_BIN"
  touch "$STAGED_DBS_BIN/.placeholder"
fi

# Skip-check also requires the dosbox-bin folder on Windows, otherwise an
# upgrade from a pre-0.6.6 checkout would leave the DLL bundle un-staged
# even though `dosbox-staging-x86_64-pc-windows-msvc.exe` is already present.
NEEDS_DBS_BIN_STAGE=0
if [[ "$OS" == MINGW* || "$OS" == MSYS* || "$OS" == CYGWIN* ]]; then
  [[ ! -f "$STAGED_DBS_BIN/dosbox-staging.exe" ]] && NEEDS_DBS_BIN_STAGE=1
fi

# The staged-shaders check requires the mandatory fallback shader FILE, not
# just the directory: a leftover .placeholder staging (transient DMG mount
# failure, CI stub) would otherwise satisfy the skip forever and ship
# shaderless bundles that crash DOSBox on fresh installs (crt-auto default).
MANDATORY_SHADER="$STAGED_SHADERS/interpolation/bilinear.glsl"
MANDATORY_PRESET="$STAGED_PRESETS/crt/crt-hyllian/hercules.preset"
MANDATORY_KEYBOARD="$STAGED_KEYBOARD/KEYBOARD.SYS"
MANDATORY_CPI="$STAGED_CPI/EGA.CPI"

# Version stamp: a VERSION bump must re-download instead of silently keeping
# the previously installed binary (dev/CI version drift).
VERSION_STAMP="$BINARIES_DIR/.dosbox-version"
STAMPED_VERSION="$(cat "$VERSION_STAMP" 2>/dev/null || true)"

if [[ -f "$OUT_BIN" && -f "$MANDATORY_SHADER" && -f "$MANDATORY_PRESET" \
      && -f "$MANDATORY_KEYBOARD" && -f "$MANDATORY_CPI" \
      && "$STAMPED_VERSION" == "$VERSION" \
      && "$NEEDS_DBS_BIN_STAGE" -eq 0 && "$FORCE" -eq 0 ]]; then
  echo "dosbox-staging-$TRIPLE $VERSION already present, skipping download."
else
  if [[ -f "$OUT_BIN" && "$FORCE" -eq 1 ]]; then
    echo "dosbox-staging-$TRIPLE already present, re-downloading (--force)."
  elif [[ -f "$OUT_BIN" && -n "$STAMPED_VERSION" && "$STAMPED_VERSION" != "$VERSION" ]]; then
    echo "dosbox-staging-$TRIPLE is $STAMPED_VERSION, want $VERSION - re-downloading."
  fi

# ── Resolve download URL ──────────────────────────────────────────────────────

BASE_URL="https://github.com/dosbox-staging/dosbox-staging/releases/download/v${VERSION}"

case "$ASSET_OS" in
  macOS)
    # Universal binary covers both Intel and ARM
    ARCHIVE="dosbox-staging-macOS-v${VERSION}.dmg"
    ;;
  Linux)
    ARCHIVE="dosbox-staging-linux-x86_64-v${VERSION}.tar.xz"
    ;;
  Linux-aarch64)
    ARCHIVE="dosbox-staging-linux-aarch64-v${VERSION}.tar.xz"
    ;;
  Windows)
    ARCHIVE="dosbox-staging-windows-x64-v${VERSION}.zip"
    ;;
esac

DOWNLOAD_URL="$BASE_URL/$ARCHIVE"

# ── Download & extract ────────────────────────────────────────────────────────

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "Downloading DOSBox Staging $VERSION for $TRIPLE..."
curl -fL --progress-bar -o "$TMP_DIR/$ARCHIVE" "$DOWNLOAD_URL"

echo "Extracting..."

# Ensure a staged dir exists as a non-empty directory even when the upstream
# archive has no such folder, so tauri build never fails on a missing
# resource path.
stage_dir_from() {
  local src="$1" dest="$2"
  mkdir -p "$(dirname "$dest")"
  rm -rf "$dest"
  if [[ -n "$src" && -d "$src" ]]; then
    cp -r "$src" "$dest"
  else
    mkdir -p "$dest"
    touch "$dest/.placeholder"
  fi
}

# Four resource dirs from the upstream `resources/` folder: `shaders` (the
# fallback among them, without which Staging aborts at startup),
# `shader-presets` (the variants crt-auto switches between - missing ones
# degrade every adaptive CRT mode), `freedos-keyboard` and `freedos-cpi`
# (a layout only loads when its code page font is there too).
stage_resources_from() {
  local shaders="$1" presets="$2" keyboard="$3" cpi="$4"
  stage_dir_from "$shaders" "$STAGED_SHADERS"
  stage_dir_from "$presets" "$STAGED_PRESETS"
  stage_dir_from "$keyboard" "$STAGED_KEYBOARD"
  stage_dir_from "$cpi" "$STAGED_CPI"
  if [[ -n "$shaders" && -d "$shaders" ]]; then
    echo "Staged shaders for bundling: $STAGED_SHADERS"
  else
    echo "WARNING: No upstream shaders found; wrote placeholder to $STAGED_SHADERS"
    echo "WARNING: A release built from this state ships WITHOUT shaders -"
    echo "         DOSBox Staging aborts on fresh installs (shader defaults to crt-auto)."
  fi
  if [[ -z "$presets" || ! -d "$presets" ]]; then
    echo "WARNING: No upstream shader presets found; crt-auto will fall back to a plain shader."
  fi
  if [[ -z "$keyboard" || ! -d "$keyboard" || -z "$cpi" || ! -d "$cpi" ]]; then
    echo "WARNING: No upstream keyboard layouts / code pages found; every game runs on the US layout."
  fi
}

# Install the same dirs into DOSBox's own config dir so local dev runs work.
install_dev_resources() {
  local dir="$1"; shift
  mkdir -p "$dir"
  local src leaf
  for src in "$@"; do
    if [[ -n "$src" && -d "$src" ]]; then
      leaf="$(basename "$src")"
      rm -rf "$dir/$leaf"
      cp -r "$src" "$dir/$leaf"
    fi
  done
  echo "Installed dev resources to $dir"
}

if [[ "$ARCHIVE" == *.dmg ]]; then
  # macOS DMG — mount, copy binary + shader resources, unmount
  MOUNT_POINT="$(mktemp -d)"
  hdiutil attach -quiet -nobrowse -mountpoint "$MOUNT_POINT" "$TMP_DIR/$ARCHIVE"
  APP="$MOUNT_POINT/DOSBox Staging.app"
  # Binary is named "dosbox" inside the .app bundle
  cp "$APP/Contents/MacOS/dosbox" "$OUT_BIN"
  # Stage + install GLSL shaders. macOS runtime uses `output = texture` in
  # launch_game, so shaders aren't strictly needed, but staging keeps the
  # bundle consistent across platforms.
  # Copy sources out of the mount before detaching.
  for leaf in shaders shader-presets freedos-keyboard freedos-cpi; do
    if [[ -d "$APP/Contents/Resources/$leaf" ]]; then
      cp -r "$APP/Contents/Resources/$leaf" "$TMP_DIR/$leaf"
    fi
  done
  hdiutil detach -quiet "$MOUNT_POINT"
  rm -rf "$MOUNT_POINT"
  SHADER_TMP="$TMP_DIR/shaders"
  PRESET_TMP="$TMP_DIR/shader-presets"
  KEYBOARD_TMP="$TMP_DIR/freedos-keyboard"
  CPI_TMP="$TMP_DIR/freedos-cpi"
  stage_resources_from "$SHADER_TMP" "$PRESET_TMP" "$KEYBOARD_TMP" "$CPI_TMP"
  if [[ -d "$SHADER_TMP" ]]; then
    install_dev_resources "$HOME/Library/Preferences/DOSBox" "$SHADER_TMP" "$PRESET_TMP" "$KEYBOARD_TMP" "$CPI_TMP"
  fi
elif [[ "$ARCHIVE" == *.tar.xz ]]; then
  tar -xJf "$TMP_DIR/$ARCHIVE" -C "$TMP_DIR"
  # Upstream ships the Linux binary as plain "dosbox" at the archive root;
  # older releases used "dosbox-staging". Accept both, skip man pages & scripts.
  FOUND="$(find "$TMP_DIR" -type f \( -name "dosbox-staging" -o -name "dosbox" \) -not -name "*.sh" -not -name "*.1" | head -1)"
  cp "$FOUND" "$OUT_BIN"
  # Stage GLSL shaders for bundling AND install to the user config dir so
  # local dev works. Without these, DOSBox aborts with
  # "Fallback shader 'interpolation/bilinear' not found".
  SHADER_SRC="$(find "$TMP_DIR" -type d -name "shaders" | head -1)"
  PRESET_SRC="$(find "$TMP_DIR" -type d -name "shader-presets" | head -1)"
  KEYBOARD_SRC="$(find "$TMP_DIR" -type d -name "freedos-keyboard" | head -1)"
  CPI_SRC="$(find "$TMP_DIR" -type d -name "freedos-cpi" | head -1)"
  stage_resources_from "$SHADER_SRC" "$PRESET_SRC" "$KEYBOARD_SRC" "$CPI_SRC"
  if [[ -n "$SHADER_SRC" ]]; then
    install_dev_resources "${XDG_CONFIG_HOME:-$HOME/.config}/dosbox" "$SHADER_SRC" "$PRESET_SRC" "$KEYBOARD_SRC" "$CPI_SRC"
  fi
elif [[ "$ARCHIVE" == *.zip ]]; then
  unzip -q "$TMP_DIR/$ARCHIVE" -d "$TMP_DIR/extracted"
  # Upstream ships the Windows binary as "dosbox.exe" at the archive root;
  # older releases used "dosbox-staging.exe". Accept both, skip the debugger build.
  FOUND="$(find "$TMP_DIR/extracted" -type f \( -name "dosbox-staging.exe" -o -name "dosbox.exe" \) -not -name "*debugger*" | head -1)"
  cp "$FOUND" "$OUT_BIN"
  # Stage GLSL shaders for bundling AND install to the user config dir.
  # %LOCALAPPDATA%\DOSBox\shaders is where DOSBox looks; without these,
  # it aborts with "Fallback shader 'interpolation/bilinear' not found".
  SHADER_SRC="$(find "$TMP_DIR/extracted" -type d -name "shaders" | head -1)"
  PRESET_SRC="$(find "$TMP_DIR/extracted" -type d -name "shader-presets" | head -1)"
  KEYBOARD_SRC="$(find "$TMP_DIR/extracted" -type d -name "freedos-keyboard" | head -1)"
  CPI_SRC="$(find "$TMP_DIR/extracted" -type d -name "freedos-cpi" | head -1)"
  stage_resources_from "$SHADER_SRC" "$PRESET_SRC" "$KEYBOARD_SRC" "$CPI_SRC"
  if [[ -n "$SHADER_SRC" ]]; then
    install_dev_resources "${LOCALAPPDATA:-$HOME/AppData/Local}/DOSBox" "$SHADER_SRC" "$PRESET_SRC" "$KEYBOARD_SRC" "$CPI_SRC"
  fi
  # Stage the full DOSBox folder so we can ship its DLLs + resources/ next
  # to the .exe in our bundle. Without SDL2.dll, vcruntime140.dll, the
  # codepage tables under resources/mapping/, etc., dosbox-staging.exe
  # fails to launch with "missing dependency" errors on Windows.
  DBS_FOLDER="$(find "$TMP_DIR/extracted" -maxdepth 2 -type d -name 'dosbox-staging-v*' | head -1)"
  if [[ -n "$DBS_FOLDER" && -d "$DBS_FOLDER" ]]; then
    STAGED_DBS="$REPO_ROOT/src-tauri/resources/dosbox-bin"
    rm -rf "$STAGED_DBS"
    mkdir -p "$STAGED_DBS"
    cp -r "$DBS_FOLDER"/. "$STAGED_DBS/"
    # Normalize the binary name: at runtime resolve_dosbox looks for
    # `dosbox-staging.exe` (matches the externalBin convention), but the
    # upstream zip names it `dosbox.exe`. Rename so the bundled folder
    # matches the lookup.
    if [[ -f "$STAGED_DBS/dosbox.exe" && ! -f "$STAGED_DBS/dosbox-staging.exe" ]]; then
      mv "$STAGED_DBS/dosbox.exe" "$STAGED_DBS/dosbox-staging.exe"
    fi
    # Drop the debugger build — confuses launch logic and bloats bundle.
    rm -f "$STAGED_DBS"/dosbox*debugger*.exe
    echo "Staged DOSBox folder for bundling: $STAGED_DBS"
  else
    echo "Warning: could not locate dosbox-staging-v* folder in upstream zip"
  fi
fi

  chmod +x "$OUT_BIN"
  echo "$VERSION" > "$VERSION_STAMP"
  echo "Installed: $OUT_BIN ($VERSION)"

  # GPL compliance: ship DOSBox Staging's license text alongside the bundled
  # binary. Prefer the copy inside the release archive; fall back to the
  # upstream repo at the same version tag.
  LICENSE_DEST="$REPO_ROOT/src-tauri/resources/dosbox-bin/LICENSE-dosbox-staging.txt"
  mkdir -p "$(dirname "$LICENSE_DEST")"
  LICENSE_SRC="$(find "$TMP_DIR" -maxdepth 4 -type f \( -name "LICENSE" -o -name "COPYING" \) 2>/dev/null | head -1)"
  if [[ -n "$LICENSE_SRC" ]]; then
    cp "$LICENSE_SRC" "$LICENSE_DEST"
    echo "Staged DOSBox Staging license: $LICENSE_DEST"
  elif [[ ! -f "$LICENSE_DEST" ]]; then
    curl -fsSL "https://raw.githubusercontent.com/dosbox-staging/dosbox-staging/v$VERSION/LICENSE" -o "$LICENSE_DEST" \
      && echo "Downloaded DOSBox Staging license: $LICENSE_DEST" \
      || echo "Warning: could not stage DOSBox Staging LICENSE (GPL compliance)"
  fi
fi

# macOS: strip quarantine and re-sign with an ad-hoc signature.
# Required because extracting the binary from the .app bundle orphans the original
# bundle-anchored code signature (references a now-missing Info.plist), causing
# macOS to SIGKILL the process on launch. Runs on every invocation (idempotent).
if [[ "$OS" == "Darwin" ]]; then
  xattr -cr "$OUT_BIN" 2>/dev/null || true
  codesign --force --sign - "$OUT_BIN"
  echo "Re-signed and quarantine cleared: $OUT_BIN"
fi
