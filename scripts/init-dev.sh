#!/usr/bin/env bash
# init-dev.sh — One-time dev setup: download metadata ZIPs and generate thumbnails.
#
# Usage:
#   pnpm run init-dev              # interactive: prompts for language packs
#   pnpm run init-dev --force      # regenerate thumbnails even if already present
#   pnpm run init-dev --glp        # also download German language pack (~23 GB)
#   pnpm run init-dev --slp        # also download Spanish language pack (~3.8 GB)
#   pnpm run init-dev --plp        # also download Polish language pack (~800 MB)
#   pnpm run init-dev --all-packs  # download all language packs
#
# Environment:
#   XDO_DEV_DATA   Override the data directory (default: ~/.exodian-dev)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ── Parse flags ───────────────────────────────────────────────────────────────

FORCE=0
WANT_GLP=0
WANT_SLP=0
WANT_PLP=0
PACKS_EXPLICIT=0   # set to 1 if any pack flag was passed (skip interactive prompt)

for arg in "$@"; do
  case "$arg" in
    --force)     FORCE=1 ;;
    --glp)       WANT_GLP=1; PACKS_EXPLICIT=1 ;;
    --slp)       WANT_SLP=1; PACKS_EXPLICIT=1 ;;
    --plp)       WANT_PLP=1; PACKS_EXPLICIT=1 ;;
    --all-packs) WANT_GLP=1; WANT_SLP=1; WANT_PLP=1; PACKS_EXPLICIT=1 ;;
  esac
done

# ── Early-exit if thumbnails are already present ──────────────────────────────

THUMB_DIR="$REPO_ROOT/thumbnails/eXoDOS"
THUMB_COUNT=0
if [[ -d "$THUMB_DIR" ]]; then
  THUMB_COUNT=$(ls "$THUMB_DIR" | wc -l | tr -d ' ')
fi

if [[ "$FORCE" -eq 0 && "$THUMB_COUNT" -gt 100 ]]; then
  echo "Thumbnails already present ($THUMB_COUNT files). Use --force to regenerate."
  exit 0
fi

# ── Prerequisite checks ───────────────────────────────────────────────────────

check_cmd() {
  if ! command -v "$1" &>/dev/null; then
    echo "ERROR: '$1' not found."
    echo "  macOS:  $2"
    echo "  Linux:  $3"
    exit 1
  fi
}

check_cmd aria2c \
  "brew install aria2" \
  "sudo apt install aria2  (or equivalent)"

check_cmd python3 \
  "brew install python3" \
  "sudo apt install python3"

# Prefer Python 3.11+ — Python 3.10 has a zipfile seek bug with files >4 GB on macOS.
PYTHON=python3
for candidate in python3.14 python3.13 python3.12 python3.11; do
  if command -v "$candidate" &>/dev/null \
      && "$candidate" -c "import sys; sys.exit(0 if sys.version_info >= (3,11) else 1)" 2>/dev/null \
      && "$candidate" -c "import PIL" 2>/dev/null; then
    PYTHON="$candidate"
    break
  fi
done

if ! $PYTHON -c "import PIL" 2>/dev/null; then
  echo "ERROR: Python 'Pillow' package not found for $PYTHON."
  echo "  Install: pip3 install Pillow  (or pip install Pillow for $PYTHON)"
  exit 1
fi

# ── Interactive language pack prompt (only when TTY, no explicit pack flags) ──

if [[ "$PACKS_EXPLICIT" -eq 0 && -t 0 ]]; then
  echo ""
  echo "Language pack thumbnails (optional extra downloads):"
  read -r -p "  GLP — German   (~23 GB):  download? [y/N] " ans
  [[ "$ans" =~ ^[Yy] ]] && WANT_GLP=1
  read -r -p "  SLP — Spanish  (~3.8 GB): download? [y/N] " ans
  [[ "$ans" =~ ^[Yy] ]] && WANT_SLP=1
  read -r -p "  PLP — Polish   (~800 MB): download? [y/N] " ans
  [[ "$ans" =~ ^[Yy] ]] && WANT_PLP=1
  echo ""
fi

# ── Helper: download a single file from a torrent ─────────────────────────────

# download_torrent_file <torrent> <file_index> <dir> <expected_zip>
download_torrent_file() {
  local torrent="$1" file_idx="$2" dir="$3" zip_path="$4"

  if [[ ! -s "$zip_path" ]]; then
    echo "  Saving to: $dir"
    mkdir -p "$dir"
    rm -f "$dir/../eXoDOS.aria2"   # remove stale control file if present
    aria2c \
      --select-file="$file_idx" \
      --seed-time=0 \
      --file-allocation=none \
      --allow-overwrite=true \
      --dir="$dir" \
      "$torrent"
    echo ""
  fi

  if [[ ! -s "$zip_path" ]]; then
    echo "ERROR: Download completed but $zip_path is missing or empty."
    echo "  No seeders may be available right now — try again later."
    exit 1
  fi
}

# ── Download eXoDOS metadata (box art source — thumbnails only) ───────────────

DATA_DIR="${XDO_DEV_DATA:-$HOME/.exodian-dev}"
METADATA_ZIP="$DATA_DIR/eXoDOS/Content/XODOSMetadata.zip"
TORRENT_EXODOS="$REPO_ROOT/torrents/eXoDOS.torrent"

echo "── eXoDOS metadata ──────────────────────────────────────────────────────────"
if [[ ! -s "$METADATA_ZIP" ]]; then
  echo "Downloading XODOSMetadata.zip (~5 GB, one-time)..."
  rm -f "$DATA_DIR/eXoDOS.aria2"
  rm -f "$METADATA_ZIP"
  download_torrent_file "$TORRENT_EXODOS" 9 "$DATA_DIR" "$METADATA_ZIP"
else
  echo "XODOSMetadata.zip already present, skipping."
fi

# ── Download language pack metadata (optional) ────────────────────────────────

GLP_ZIP="$DATA_DIR/eXoDOS_GLP/eXoDOS/Content/eXoDOS_GLP_Metadata.zip"
SLP_ZIP="$DATA_DIR/eXoDOS_SLP/eXoDOS/Content/eXoDOS_SLP_Metadata.zip"
PLP_ZIP="$DATA_DIR/eXoDOS_PLP/eXoDOS/Content/eXoDOS_PLP_Metadata.zip"

if [[ "$WANT_GLP" -eq 1 ]]; then
  echo "── GLP (German) metadata ────────────────────────────────────────────────────"
  if [[ ! -s "$GLP_ZIP" ]]; then
    echo "Downloading eXoDOS_GLP_Metadata.zip (~23 GB)..."
    download_torrent_file "$REPO_ROOT/torrents/eXoDOS_GLP.torrent" 5 "$DATA_DIR/eXoDOS_GLP" "$GLP_ZIP"
  else
    echo "eXoDOS_GLP_Metadata.zip already present, skipping."
  fi
fi

if [[ "$WANT_SLP" -eq 1 ]]; then
  echo "── SLP (Spanish) metadata ───────────────────────────────────────────────────"
  if [[ ! -s "$SLP_ZIP" ]]; then
    echo "Downloading eXoDOS_SLP_Metadata.zip (~3.8 GB)..."
    download_torrent_file "$REPO_ROOT/torrents/eXoDOS_SLP.torrent" 1 "$DATA_DIR/eXoDOS_SLP" "$SLP_ZIP"
  else
    echo "eXoDOS_SLP_Metadata.zip already present, skipping."
  fi
fi

if [[ "$WANT_PLP" -eq 1 ]]; then
  echo "── PLP (Polish) metadata ────────────────────────────────────────────────────"
  if [[ ! -s "$PLP_ZIP" ]]; then
    echo "Downloading eXoDOS_PLP_Metadata.zip (~800 MB)..."
    download_torrent_file "$REPO_ROOT/torrents/eXoDOS_PLP.torrent" 3 "$DATA_DIR/eXoDOS_PLP" "$PLP_ZIP"
  else
    echo "eXoDOS_PLP_Metadata.zip already present, skipping."
  fi
fi

# ── Generate thumbnails ───────────────────────────────────────────────────────

FORCE_FLAG=""
if [[ "$FORCE" -eq 1 ]]; then FORCE_FLAG="--force"; fi

mkdir -p "$THUMB_DIR"

echo "── Generating thumbnails ────────────────────────────────────────────────────"
echo "eXoDOS (EN)..."
$PYTHON "$SCRIPT_DIR/gen_thumbnails.py" \
  "$METADATA_ZIP" \
  "$REPO_ROOT/metadata/MS-DOS.xml.gz" \
  "$THUMB_DIR" \
  $FORCE_FLAG

if [[ "$WANT_GLP" -eq 1 && -s "$GLP_ZIP" ]]; then
  echo "GLP (German)..."
  $PYTHON "$SCRIPT_DIR/gen_thumbnails.py" \
    "$GLP_ZIP" \
    "$REPO_ROOT/metadata/GLP.xml.gz" \
    "$THUMB_DIR" \
    $FORCE_FLAG
fi

if [[ "$WANT_SLP" -eq 1 && -s "$SLP_ZIP" ]]; then
  echo "SLP (Spanish)..."
  $PYTHON "$SCRIPT_DIR/gen_thumbnails.py" \
    "$SLP_ZIP" \
    "$REPO_ROOT/metadata/SLP.xml.gz" \
    "$THUMB_DIR" \
    $FORCE_FLAG
fi

if [[ "$WANT_PLP" -eq 1 && -s "$PLP_ZIP" ]]; then
  echo "PLP (Polish)..."
  $PYTHON "$SCRIPT_DIR/gen_thumbnails.py" \
    "$PLP_ZIP" \
    "$REPO_ROOT/metadata/PLP.xml.gz" \
    "$THUMB_DIR" \
    $FORCE_FLAG
fi

echo ""
echo "Setup complete. Run 'pnpm tauri dev' to start."
