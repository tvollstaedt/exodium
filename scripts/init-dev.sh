#!/usr/bin/env bash
# init-dev.sh — One-time dev setup: download metadata ZIPs and generate thumbnails.
#
# Usage:
#   pnpm run init-dev              # interactive: prompts for language packs
#   pnpm run init-dev --force      # regenerate thumbnails even if already present
#   pnpm run init-dev --glp        # also download German language pack (~23 GB)
#   pnpm run init-dev --slp        # also download Spanish language pack (~3.8 GB)
#   pnpm run init-dev --plp        # also download Polish language pack (~800 MB)
#   pnpm run init-dev --win3x      # also download eXoWin3x box art (~2.3 GB)
#   pnpm run init-dev --win9x      # also download eXoWin9x metadata (~13 GB)
#   pnpm run init-dev --all-packs  # download all language packs
#
# Environment:
#   XDO_DEV_DATA   Override the data directory (default: ~/.exodium-dev)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# ── Parse flags ───────────────────────────────────────────────────────────────

FORCE=0
WANT_GLP=0
WANT_SLP=0
WANT_PLP=0
WANT_WIN3X=0
WANT_WIN9X=0
PACKS_EXPLICIT=0   # set to 1 if any pack flag was passed (skip interactive prompt)

for arg in "$@"; do
  case "$arg" in
    --force)     FORCE=1 ;;
    --glp)       WANT_GLP=1; PACKS_EXPLICIT=1 ;;
    --slp)       WANT_SLP=1; PACKS_EXPLICIT=1 ;;
    --plp)       WANT_PLP=1; PACKS_EXPLICIT=1 ;;
    --win3x)     WANT_WIN3X=1; PACKS_EXPLICIT=1 ;;
    --win9x)     WANT_WIN9X=1; PACKS_EXPLICIT=1 ;;
    --all-packs) WANT_GLP=1; WANT_SLP=1; WANT_PLP=1; WANT_WIN3X=1; WANT_WIN9X=1; PACKS_EXPLICIT=1 ;;
  esac
done

# ── Early-exit if thumbnails are already present ──────────────────────────────

THUMB_DIR="$REPO_ROOT/thumbnails/eXoDOS"
THUMB_COUNT=0
if [[ -d "$THUMB_DIR" ]]; then
  THUMB_COUNT=$(ls "$THUMB_DIR" | wc -l | tr -d ' ')
fi

# Explicit pack flags must get past this: their download sections come later,
# and the per-pack thumbnail passes are idempotent anyway.
if [[ "$FORCE" -eq 0 && "$PACKS_EXPLICIT" -eq 0 && "$THUMB_COUNT" -gt 100 ]]; then
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
  read -r -p "  eXoWin3x       (~2.3 GB): download? [y/N] " ans
  [[ "$ans" =~ ^[Yy] ]] && WANT_WIN3X=1
  read -r -p "  eXoWin9x       (~13 GB):  download? [y/N] " ans
  [[ "$ans" =~ ^[Yy] ]] && WANT_WIN9X=1
  echo ""
fi

# ── Helper: download a single file from a torrent ─────────────────────────────

# torrent_file_index <torrent> <filename> <fallback_idx>
# Resolve the aria2c --select-file index for a filename by parsing
# `aria2c --show-files` output. Hardcoded indices break when eXoDOS publishes
# a new torrent with a reshuffled file list - and the failure mode is a
# multi-GB wrong download. Falls back to the known-good index with a warning
# if parsing yields nothing (e.g. aria2c output format changes).
torrent_file_index() {
  local torrent="$1" filename="$2" fallback="$3"
  local idx
  idx="$(aria2c --show-files=true "$torrent" 2>/dev/null \
    | grep -E "^ *[0-9]+\|.*${filename}\$" \
    | head -1 \
    | sed -E 's/^ *([0-9]+)\|.*/\1/' || true)"
  if [[ "$idx" =~ ^[0-9]+$ ]]; then
    echo "$idx"
  else
    echo "  WARNING: could not resolve index for $filename in $(basename "$torrent"); using fallback $fallback" >&2
    echo "$fallback"
  fi
}

# download_torrent_file <torrent> <file_index> <dir> <expected_zip>
download_torrent_file() {
  local torrent="$1" file_idx="$2" dir="$3" zip_path="$4"

  if [[ ! -s "$zip_path" ]]; then
    echo "  Saving to: $dir"
    mkdir -p "$dir"
    rm -f "$dir/eXoDOS.aria2"       # torrent-level control file
    rm -f "$zip_path.aria2"         # piece-level control file adjacent to zip
    aria2c \
      --select-file="$file_idx" \
      --seed-time=0 \
      --file-allocation=none \
      --allow-overwrite=true \
      --dir="$dir" \
      "$torrent"
    echo ""
  fi

  if ! validate_zip "$zip_path"; then
    echo "ERROR: Download completed but $zip_path is missing or corrupt."
    echo "  No seeders may be available right now — try again later."
    exit 1
  fi
}

# validate_zip <zip_path>
# Returns 0 if the file exists and is a valid ZIP; deletes and returns 1 if corrupt.
validate_zip() {
  local zip_path="$1"
  if [[ ! -s "$zip_path" ]]; then
    return 1
  fi
  # $PYTHON, not python3: the 3.11+ selection above exists because Python
  # 3.10's zipfile mis-seeks in archives >4 GB - bare python3 here would
  # falsely flag a good 5 GB download as corrupt and delete it in a loop.
  if "$PYTHON" -c "import zipfile, sys; f=zipfile.ZipFile('$zip_path'); sys.exit(0 if f.testzip() is None else 1)" 2>/dev/null; then
    return 0
  fi
  echo "  WARNING: $zip_path is corrupt. Deleting for re-download..."
  rm -f "$zip_path" "$zip_path.aria2"
  return 1
}

# ── Download eXoDOS metadata (box art source — thumbnails only) ───────────────

DATA_DIR="${XDO_DEV_DATA:-$HOME/.exodium-dev}"
METADATA_ZIP="$DATA_DIR/eXoDOS/Content/XODOSMetadata.zip"
TORRENT_EXODOS="$REPO_ROOT/torrents/eXoDOS.torrent"

echo "── eXoDOS metadata ──────────────────────────────────────────────────────────"
if ! validate_zip "$METADATA_ZIP"; then
  echo "Downloading XODOSMetadata.zip (~5 GB, one-time)..."
  download_torrent_file "$TORRENT_EXODOS" \
    "$(torrent_file_index "$TORRENT_EXODOS" "XODOSMetadata.zip" 9)" \
    "$DATA_DIR" "$METADATA_ZIP"
else
  echo "XODOSMetadata.zip already present, skipping."
fi

# ── Download language pack metadata (optional) ────────────────────────────────

GLP_ZIP="$DATA_DIR/eXoDOS_GLP/eXoDOS/Content/eXoDOS_GLP_Metadata.zip"
SLP_ZIP="$DATA_DIR/eXoDOS_SLP/eXoDOS/Content/eXoDOS_SLP_Metadata.zip"
PLP_ZIP="$DATA_DIR/eXoDOS_PLP/eXoDOS/Content/eXoDOS_PLP_Metadata.zip"
WIN3X_ZIP="$DATA_DIR/eXoWin3x/eXoWin3x/Content/XOWin3xMetadata.zip"
WIN9X_ZIP="$DATA_DIR/eXoWin9x/eXoWin9x/Content/XOWin9xMetadata.zip"
WIN9X_CONFIGS_ZIP="$DATA_DIR/eXoWin9x/eXoWin9x/Content/!Win9Xmetadata.zip"

if [[ "$WANT_GLP" -eq 1 ]]; then
  echo "── GLP (German) metadata ────────────────────────────────────────────────────"
  if ! validate_zip "$GLP_ZIP"; then
    echo "Downloading eXoDOS_GLP_Metadata.zip (~23 GB)..."
    download_torrent_file "$REPO_ROOT/torrents/eXoDOS_GLP.torrent" \
      "$(torrent_file_index "$REPO_ROOT/torrents/eXoDOS_GLP.torrent" "eXoDOS_GLP_Metadata.zip" 5)" \
      "$DATA_DIR/eXoDOS_GLP" "$GLP_ZIP"
  else
    echo "eXoDOS_GLP_Metadata.zip already present, skipping."
  fi
fi

if [[ "$WANT_SLP" -eq 1 ]]; then
  echo "── SLP (Spanish) metadata ───────────────────────────────────────────────────"
  if ! validate_zip "$SLP_ZIP"; then
    echo "Downloading eXoDOS_SLP_Metadata.zip (~3.8 GB)..."
    download_torrent_file "$REPO_ROOT/torrents/eXoDOS_SLP.torrent" \
      "$(torrent_file_index "$REPO_ROOT/torrents/eXoDOS_SLP.torrent" "eXoDOS_SLP_Metadata.zip" 1)" \
      "$DATA_DIR/eXoDOS_SLP" "$SLP_ZIP"
  else
    echo "eXoDOS_SLP_Metadata.zip already present, skipping."
  fi
fi

if [[ "$WANT_PLP" -eq 1 ]]; then
  echo "── PLP (Polish) metadata ────────────────────────────────────────────────────"
  if ! validate_zip "$PLP_ZIP"; then
    echo "Downloading eXoDOS_PLP_Metadata.zip (~800 MB)..."
    download_torrent_file "$REPO_ROOT/torrents/eXoDOS_PLP.torrent" \
      "$(torrent_file_index "$REPO_ROOT/torrents/eXoDOS_PLP.torrent" "eXoDOS_PLP_Metadata.zip" 3)" \
      "$DATA_DIR/eXoDOS_PLP" "$PLP_ZIP"
  else
    echo "eXoDOS_PLP_Metadata.zip already present, skipping."
  fi
fi

if [[ "$WANT_WIN3X" -eq 1 ]]; then
  echo "── eXoWin3x metadata ────────────────────────────────────────────────────────"
  if ! validate_zip "$WIN3X_ZIP"; then
    echo "Downloading XOWin3xMetadata.zip (~2.3 GB)..."
    download_torrent_file "$REPO_ROOT/torrents/eXoWin3x.torrent" \
      "$(torrent_file_index "$REPO_ROOT/torrents/eXoWin3x.torrent" "XOWin3xMetadata.zip" 6)" \
      "$DATA_DIR/eXoWin3x" "$WIN3X_ZIP"
  else
    echo "XOWin3xMetadata.zip already present, skipping."
  fi
fi

if [[ "$WANT_WIN9X" -eq 1 ]]; then
  echo "── eXoWin9x metadata ────────────────────────────────────────────────────────"
  # Two zips: XOWin9xMetadata.zip carries covers + the catalogue XML,
  # !Win9Xmetadata.zip carries the per-game launch confs/bats that
  # gen_win9x_assets.py strips into the bundled Win9x_configs.zip.
  if ! validate_zip "$WIN9X_ZIP"; then
    echo "Downloading XOWin9xMetadata.zip (~4.6 GB)..."
    download_torrent_file "$REPO_ROOT/torrents/eXoWin9x.torrent" \
      "$(torrent_file_index "$REPO_ROOT/torrents/eXoWin9x.torrent" "XOWin9xMetadata.zip" 7)" \
      "$DATA_DIR/eXoWin9x" "$WIN9X_ZIP"
  else
    echo "XOWin9xMetadata.zip already present, skipping."
  fi
  if ! validate_zip "$WIN9X_CONFIGS_ZIP"; then
    echo "Downloading !Win9Xmetadata.zip (~8.4 GB)..."
    download_torrent_file "$REPO_ROOT/torrents/eXoWin9x.torrent" \
      "$(torrent_file_index "$REPO_ROOT/torrents/eXoWin9x.torrent" '!Win9Xmetadata.zip' 1)" \
      "$DATA_DIR/eXoWin9x" "$WIN9X_CONFIGS_ZIP"
  else
    echo "!Win9Xmetadata.zip already present, skipping."
  fi
  if [[ ! -s "$REPO_ROOT/metadata/Win9x.xml.gz" || "$FORCE" -eq 1 ]]; then
    echo "Regenerating bundled eXoWin9x metadata (gen_win9x_assets.py)..."
    $PYTHON "$SCRIPT_DIR/gen_win9x_assets.py"
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
  --db "$REPO_ROOT/metadata/exodium.db" \
  $FORCE_FLAG

if [[ "$WANT_GLP" -eq 1 && -s "$GLP_ZIP" ]]; then
  echo "GLP (German)..."
  # GLP zip also contains German box art for games in the EN catalog, so pass
  # MS-DOS.xml.gz as a fallback to resolve shortcodes for those images too.
  $PYTHON "$SCRIPT_DIR/gen_thumbnails.py" \
    "$GLP_ZIP" \
    "$REPO_ROOT/metadata/GLP.xml.gz" \
    "$THUMB_DIR" \
    --db "$REPO_ROOT/metadata/exodium.db" \
    --extra-xml "$REPO_ROOT/metadata/MS-DOS.xml.gz" \
    $FORCE_FLAG
fi

if [[ "$WANT_SLP" -eq 1 && -s "$SLP_ZIP" ]]; then
  echo "SLP (Spanish)..."
  # SLP.xml.gz paths use !spanish/<title>.bat with no shortcode dir; shortcodes
  # come from the DB (highest priority) and zip bat structure.
  $PYTHON "$SCRIPT_DIR/gen_thumbnails.py" \
    "$SLP_ZIP" \
    "$REPO_ROOT/metadata/SLP.xml.gz" \
    "$THUMB_DIR" \
    --db "$REPO_ROOT/metadata/exodium.db" \
    --extra-xml "$REPO_ROOT/metadata/MS-DOS.xml.gz" \
    $FORCE_FLAG
fi

if [[ "$WANT_PLP" -eq 1 && -s "$PLP_ZIP" ]]; then
  echo "PLP (Polish)..."
  # PLP.xml.gz paths use !polish/<title>.bat with no shortcode dir; shortcodes
  # come from the DB (highest priority) and zip bat structure.
  $PYTHON "$SCRIPT_DIR/gen_thumbnails.py" \
    "$PLP_ZIP" \
    "$REPO_ROOT/metadata/MS-DOS.xml.gz" \
    "$THUMB_DIR" \
    --db "$REPO_ROOT/metadata/exodium.db" \
    --extra-xml "$REPO_ROOT/metadata/PLP.xml.gz" \
    $FORCE_FLAG
fi

if [[ "$WANT_WIN3X" -eq 1 && -s "$WIN3X_ZIP" ]]; then
  echo "eXoWin3x (Windows 3.x)..."
  # Own platform subtree and own thumbnail dir - Win3x covers must not land in
  # thumbnails/eXoDOS, where a same-titled DOS game would collide with them.
  $PYTHON "$SCRIPT_DIR/gen_thumbnails.py" \
    "$WIN3X_ZIP" \
    "$REPO_ROOT/metadata/Win3x.xml.gz" \
    "$REPO_ROOT/thumbnails/eXoWin3x" \
    --preview-dir "$REPO_ROOT/src-tauri/resources/previews/eXoWin3x" \
    --platform "Windows 3x" \
    --db "$REPO_ROOT/metadata/exodium.db" \
    $FORCE_FLAG
fi

if [[ "$WANT_WIN9X" -eq 1 && -s "$WIN9X_ZIP" ]]; then
  echo "eXoWin9x (Windows 9x)..."
  # Own platform subtree and own thumbnail dir, like eXoWin3x.
  $PYTHON "$SCRIPT_DIR/gen_thumbnails.py" \
    "$WIN9X_ZIP" \
    "$REPO_ROOT/metadata/Win9x.xml.gz" \
    "$REPO_ROOT/thumbnails/eXoWin9x" \
    --preview-dir "$REPO_ROOT/src-tauri/resources/previews/eXoWin9x" \
    --platform "Windows 9x" \
    --db "$REPO_ROOT/metadata/exodium.db" \
    $FORCE_FLAG
fi

echo ""
echo "Setup complete. Run 'pnpm tauri dev' to start."
