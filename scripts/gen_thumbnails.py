#!/usr/bin/env python3
"""
Generate shortcode-keyed thumbnail JPEGs from an eXoDOS metadata ZIP.

Works for the main eXoDOS pack and all language packs (GLP/SLP/PLP).
Each pack uses platform 'MS-DOS' so image paths are always
Images/MS-DOS/Box - Front/<title>-NN.jpg inside the zip.

Usage:
    python3 scripts/gen_thumbnails.py <metadata_zip> <xml_gz> <output_dir> [--force]
                                       [--extra-xml <xml_gz>]

    metadata_zip   Path to XODOSMetadata.zip / eXoDOS_GLP_Metadata.zip / etc.
    xml_gz         Matching bundled catalogue: metadata/MS-DOS.xml.gz, GLP.xml.gz, etc.
    output_dir     Destination for shortcode-keyed JPEGs (e.g. thumbnails/eXoDOS)
    --force        Overwrite already-existing thumbnails
    --db           Path to metadata/exodium.db — used as highest-priority shortcode
                   source so LP-exclusive thumbnails get the correct generate_shortcode()
                   name (e.g. ElDesafi.jpg) rather than the zip bat directory name
    --extra-xml    Additional XML catalogue to merge as fallback (e.g. MS-DOS.xml.gz
                   for GLP, which includes German box art for EN-catalog games)

Dependencies: Pillow  (pip3 install Pillow)
"""

import gzip
import io
import re
import sqlite3
import sys
import unicodedata
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET

try:
    from PIL import Image
except ImportError:
    print("ERROR: Pillow not installed. Run: pip3 install Pillow")
    sys.exit(1)


_ROMAN_VALS: dict[str, int] = {"i": 1, "v": 5, "x": 10, "l": 50, "c": 100}
_TEIL_RE = re.compile(r"\bTeil\s+([IVXivx]+)(?![A-Za-z])")
# Standalone Roman numerals, excluding bare "I" (=1) which causes too many false-positives.
# "V" (=5) and above are included — e.g. "Street Fighter V" → "Street Fighter 5" aids matching.
# Matches only when not adjacent to other letters.
_ROMAN_NUM_RE = re.compile(
    r"(?<![A-Za-z])"
    r"(X{1,3}(?:IX|IV|V?I{0,3})|IX|IV|VI{0,3}|II{1,2})"
    r"(?![A-Za-z])"
)
_SUPERSCRIPT: dict[str, str] = {"¹": "1", "²": "2", "³": "3",
                                 "⁴": "4", "⁵": "5", "⁶": "6",
                                 "⁷": "7", "⁸": "8", "⁹": "9", "⁰": "0"}
# Latin-script diacritic/special-char expansions (German umlauts, Polish ł, etc.)
_UMLAUT: dict[str, str] = {"ä": "ae", "ö": "oe", "ü": "ue",
                            "Ä": "ae", "Ö": "oe", "Ü": "ue", "ß": "ss",
                            # Polish ł/Ł is not NFD-decomposable; map explicitly
                            "ł": "l", "Ł": "l"}
# Separators that mark the boundary between a main title and an optional subtitle
_SUBTITLE_SEPS = re.compile(r"\s+[-_]\s+|\s*[_(]")


def _roman_to_int(s: str) -> int:
    s = s.lower()
    total = 0
    for i, c in enumerate(s):
        val = _ROMAN_VALS.get(c, 0)
        if i + 1 < len(s) and _ROMAN_VALS.get(s[i + 1], 0) > val:
            total -= val
        else:
            total += val
    return total


def prenormalize(text: str) -> str:
    """Expand superscript digits and convert Roman numerals to Arabic."""
    for ch, rep in _SUPERSCRIPT.items():
        text = text.replace(ch, rep)
    # Convert 'Teil I/II/III' first (same as before)
    text = _TEIL_RE.sub(lambda m: str(_roman_to_int(m.group(1))), text)
    # Convert any remaining standalone Roman numeral >= 2 (e.g. "Larry III" → "Larry 3")
    text = _ROMAN_NUM_RE.sub(lambda m: str(_roman_to_int(m.group(1))), text)
    return text


def normalize(text: str) -> str:
    """Expand German umlauts (ä→ae etc.), strip remaining accents, drop non-alnum."""
    for ch, rep in _UMLAUT.items():
        text = text.replace(ch, rep)
    text = unicodedata.normalize("NFD", text)
    text = "".join(c for c in text if unicodedata.category(c) != "Mn")
    return re.sub(r"[^a-z0-9]", "", text.lower())


def article_swap(title: str) -> str | None:
    """
    LaunchBox stores titles as "Word, Article" (e.g. "Amt, Das", "Office, The").
    Returns the natural-order form ("Das Amt") so we can index both directions.
    Returns None if the title has no comma convention to swap.
    """
    idx = title.rfind(", ")
    if idx == -1:
        return None
    article = title[idx + 2:]
    base = title[:idx]
    return article + " " + base


def title_variants(title: str) -> list[str]:
    """
    Return progressively shorter forms of a title to use as fallback lookup keys.
    Order: full title, then subtitle-stripped form.
    Article-swapped variants are handled separately by callers via article_swap().
    """
    variants: list[str] = [title]
    m = _SUBTITLE_SEPS.search(title)
    if m and m.start() > 0:
        variants.append(title[: m.start()].strip())
    return variants


_BAT_RE = re.compile(r"eXo/eXoDOS/!dos/![^/]+/([^/]+)/(.+)\.bat$", re.IGNORECASE)
_SKIP_STEMS = {"install", "alternate launcher"}
_YEAR_SUFFIX = re.compile(r"\s*\(\d{4}\)\s*$")


def build_title_to_shortcode_from_zip(zip_path: str) -> dict[str, str]:
    """
    Build a title→shortcode map from the zip's internal game directory structure.

    Scans bat files at eXo/eXoDOS/!dos/!<lang>/<SC>/<Title> (Year).bat and maps
    normalize(title) → SC.  Covers language-pack-exclusive games whose XML
    ApplicationPath uses a flat !<lang>/<title>.bat path with no shortcode dir
    (SLP, PLP).  Has no effect on packs whose XML already provides shortcodes.
    """
    mapping: dict[str, str] = {}
    with zipfile.ZipFile(zip_path, "r") as zf:
        for name in zf.namelist():
            m = _BAT_RE.match(name)
            if m is None:
                continue
            sc, bat_stem = m.group(1), m.group(2)
            if bat_stem.lower() in _SKIP_STEMS or "/Extras/" in name:
                continue
            title = _YEAR_SUFFIX.sub("", bat_stem).strip()
            if not title:
                continue
            for variant in title_variants(title):
                mapping.setdefault(normalize(prenormalize(variant)), sc)
                sw = article_swap(variant)
                if sw:
                    mapping.setdefault(normalize(prenormalize(sw)), sc)
    return mapping


def build_title_to_shortcode_from_db(db_path: str) -> dict[str, str]:
    """
    Query the pre-built exodium.db for title→shortcode mappings.
    This is the highest-priority source because generate_db.rs is the authoritative
    shortcode generator (including LP-exclusive games via generate_shortcode()).
    Returns an empty dict if the DB is empty or has not been generated yet.
    """
    mapping: dict[str, str] = {}
    conn = sqlite3.connect(db_path)
    try:
        for title, shortcode in conn.execute(
            "SELECT title, shortcode FROM games WHERE shortcode IS NOT NULL AND shortcode != ''"
        ):
            for variant in title_variants(title):
                mapping.setdefault(normalize(prenormalize(variant)), shortcode)
                sw = article_swap(variant)
                if sw:
                    mapping.setdefault(normalize(prenormalize(sw)), shortcode)
    except sqlite3.OperationalError as e:
        print(f"  WARNING: could not read {db_path} ({e}). "
              f"Run 'cargo run --bin generate_db' to rebuild it.")
    finally:
        conn.close()
    return mapping


def build_title_to_shortcode(xml_gz_path: str) -> dict[str, str]:
    """
    Parse an XML catalogue and return a map of normalized_title → shortcode.
    Shortcode is extracted from ApplicationPath: eXo\\eXoDOS\\<SC>\\dosbox.conf
    Both the canonical LaunchBox title ("Amt, Das") and the natural-order form
    ("Das Amt") are indexed so that image files using either convention match.
    """
    mapping: dict[str, str] = {}
    with gzip.open(xml_gz_path, "rb") as f:
        tree = ET.parse(f)

    for game in tree.findall(".//Game"):
        title_el = game.find("Title")
        app_path_el = game.find("ApplicationPath")
        if title_el is None or app_path_el is None:
            continue
        title = (title_el.text or "").strip()
        app_path = (app_path_el.text or "").replace("\\", "/")
        # Path format: eXo/eXoDOS/!dos/<shortcode>/game.bat
        # (possibly with a language dir: eXo/eXoDOS/!dos/!german/<shortcode>/game.bat)
        dos_idx = app_path.find("/!dos/")
        if dos_idx == -1:
            continue
        after_dos = app_path[dos_idx + 6:]
        # Skip language sub-dir (starts with !)
        if after_dos.startswith("!"):
            lang_end = after_dos.find("/")
            if lang_end == -1:
                continue
            after_dos = after_dos[lang_end + 1:]
        sc_end = after_dos.find("/")
        shortcode = after_dos[:sc_end] if sc_end != -1 else after_dos
        if shortcode and title:
            for variant in title_variants(title):
                mapping.setdefault(normalize(prenormalize(variant)), shortcode)
                swapped = article_swap(variant)
                if swapped:
                    mapping.setdefault(normalize(prenormalize(swapped)), shortcode)

    return mapping


def lookup(title_to_sc: dict[str, str], title: str) -> str | None:
    """Try all title variants (full, subtitle-stripped) and article-swapped forms."""
    for variant in title_variants(title):
        sc = title_to_sc.get(normalize(prenormalize(variant)))
        if sc:
            return sc
        swapped = article_swap(variant)
        if swapped:
            sc = title_to_sc.get(normalize(prenormalize(swapped)))
            if sc:
                return sc
    return None


def image_stem_to_title(filename: str) -> str:
    """
    LaunchBox images are named like 'Space Quest V- The Next Mutation-01.png'.
    Strip the trailing '-NN' counter to recover the game title.
    """
    stem = Path(filename).stem  # "Space Quest V- The Next Mutation-01"
    # Remove trailing -NN or _NN numeric suffix (1-2 digits)
    stem = re.sub(r"[-_]\d{1,2}$", "", stem)
    return stem


def main() -> None:
    extra_xml: str | None = None
    db_path: str | None = None
    raw_args = sys.argv[1:]

    if "--extra-xml" in raw_args:
        idx = raw_args.index("--extra-xml")
        if idx + 1 >= len(raw_args):
            print("Error: --extra-xml requires a path argument")
            sys.exit(1)
        extra_xml = raw_args[idx + 1]
        raw_args = raw_args[:idx] + raw_args[idx + 2:]

    preview_dir: Path | None = None
    if "--preview-dir" in raw_args:
        idx = raw_args.index("--preview-dir")
        if idx + 1 >= len(raw_args):
            print("Error: --preview-dir requires a path argument")
            sys.exit(1)
        preview_dir = Path(raw_args[idx + 1])
        preview_dir.mkdir(parents=True, exist_ok=True)
        raw_args = raw_args[:idx] + raw_args[idx + 2:]

    if "--db" in raw_args:
        idx = raw_args.index("--db")
        if idx + 1 >= len(raw_args):
            print("Error: --db requires a path argument")
            sys.exit(1)
        db_path = raw_args[idx + 1]
        raw_args = raw_args[:idx] + raw_args[idx + 2:]

    args = [a for a in raw_args if not a.startswith("-")]
    force = "--force" in raw_args

    if len(args) != 3:
        print(__doc__)
        sys.exit(1)

    metadata_zip_path, xml_gz_path, output_dir_str = args
    output_dir = Path(output_dir_str)
    output_dir.mkdir(parents=True, exist_ok=True)

    # Priority order (highest first):
    # 1. DB shortcodes (authoritative — generate_db.rs is the source of truth)
    # 2. Zip bat structure (covers LP-exclusive games not in the primary XML)
    # 3. Primary XML catalogue
    # 4. Extra XML catalogue (fallback)

    title_to_sc: dict[str, str] = {}

    if db_path:
        print(f"Loading shortcodes from {db_path}...")
        title_to_sc = build_title_to_shortcode_from_db(db_path)
        print(f"  {len(title_to_sc)} keys from DB (authoritative)")

    zip_map = build_title_to_shortcode_from_zip(metadata_zip_path)
    before = len(title_to_sc)
    for k, v in zip_map.items():
        title_to_sc.setdefault(k, v)
    print(f"  +{len(title_to_sc) - before} additional keys from zip structure")

    print(f"Parsing {xml_gz_path} for title→shortcode mapping...")
    xml_map = build_title_to_shortcode(xml_gz_path)
    before = len(title_to_sc)
    for k, v in xml_map.items():
        title_to_sc.setdefault(k, v)
    print(f"  +{len(title_to_sc) - before} additional keys from XML")

    if extra_xml:
        print(f"Merging extra catalogue {extra_xml}...")
        extra_map = build_title_to_shortcode(extra_xml)
        before = len(title_to_sc)
        for k, v in extra_map.items():
            title_to_sc.setdefault(k, v)
        print(f"  +{len(title_to_sc) - before} additional keys")

    print(f"Opening {metadata_zip_path}...")
    with zipfile.ZipFile(metadata_zip_path, "r") as zf:
        box_front = [
            n for n in zf.namelist()
            if n.startswith("Images/MS-DOS/Box - Front/")
            and n.lower().endswith((".png", ".jpg", ".jpeg"))
        ]
        print(f"  {len(box_front)} box-front images found")

        matched = 0
        skipped = 0
        unmatched: list[str] = []

        for i, name in enumerate(box_front):
            if i > 0 and i % 500 == 0:
                print(f"  {i}/{len(box_front)}  matched={matched}  skipped={skipped}  unmatched={len(unmatched)}")

            title = image_stem_to_title(name)
            shortcode = lookup(title_to_sc, title)

            if not shortcode:
                unmatched.append(name)
                continue

            out_path = output_dir / f"{shortcode}.jpg"
            if out_path.exists() and not force:
                skipped += 1
                continue

            with zf.open(name) as f:
                try:
                    img = Image.open(io.BytesIO(f.read()))
                    img = img.convert("RGB")
                    w, h = img.size
                    new_w = 400
                    new_h = max(1, int(h * new_w / w))
                    img = img.resize((new_w, new_h), Image.LANCZOS)
                    img.save(out_path, "JPEG", quality=90, optimize=True)
                    matched += 1
                    # Generate Tier 0 low-quality preview alongside full-size.
                    if preview_dir is not None:
                        preview_path = preview_dir / f"{shortcode}.jpg"
                        if not preview_path.exists() or force:
                            pw = 80
                            ph = max(1, int(h * pw / w))
                            preview = img.resize((pw, ph), Image.LANCZOS)
                            preview.save(preview_path, "JPEG", quality=40, optimize=True)
                except Exception as e:
                    print(f"  WARN: failed to process {name}: {e}")

    total = matched + skipped
    print(f"\nDone: {total} thumbnails in {output_dir}")
    print(f"  New: {matched}  Already existed: {skipped}  Unmatched: {len(unmatched)}")
    if unmatched:
        print(f"  First 10 unmatched: {unmatched[:10]}")


if __name__ == "__main__":
    main()
