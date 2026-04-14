#!/usr/bin/env python3
"""
Generate content-addressed thumbnail JPEGs from an eXoDOS metadata ZIP.

Filenames are SHA-256(normalized title)[:16] so the identifier is stable,
collision-proof, and independent of shortcode naming. The same hash function
is implemented in src-tauri/src/bin/generate_db.rs::thumbnail_key — both sides
must agree or the frontend lookup will miss.

Works for the main eXoDOS pack and all language packs (GLP/SLP/PLP).
Each pack uses platform 'MS-DOS' so image paths are always
Images/MS-DOS/Box - Front/<title>-NN.jpg inside the zip.

Usage:
    python3 scripts/gen_thumbnails.py <metadata_zip> <xml_gz> <output_dir> [--force]
                                       [--extra-xml <xml_gz>]

    metadata_zip   Path to XODOSMetadata.zip / eXoDOS_GLP_Metadata.zip / etc.
    xml_gz         Matching bundled catalogue: metadata/MS-DOS.xml.gz, GLP.xml.gz, etc.
    output_dir     Destination for <hash>.jpg files (e.g. thumbnails/eXoDOS)
    --force        Overwrite already-existing thumbnails
    --db           Path to metadata/exodium.db — used as highest-priority title
                   source because generate_db.rs imports the canonical XML title
    --extra-xml    Additional XML catalogue to merge as fallback (e.g. MS-DOS.xml.gz
                   for GLP, which includes German box art for EN-catalog games)

Dependencies: Pillow  (pip3 install Pillow)
"""

import gzip
import hashlib
import io
import re
import sqlite3
import sys
import unicodedata
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET


def thumbnail_key(title: str) -> str:
    """
    SHA-256(alnum-only lowercase title)[:16] — the content-addressed filename
    stem. Must match src-tauri/src/db/mod.rs::title_thumbnail_key and
    src-tauri/src/bin/generate_db.rs::thumbnail_key exactly.

    The stripped-alnum rule merges punctuation variants ("3-K Trivia",
    "3K Trivia", "3, K. Trivia!" all hash the same) so trivial drift between
    XML / zip / image filenames doesn't break lookup.
    """
    norm = "".join(c for c in title.lower() if c.isascii() and c.isalnum())
    return hashlib.sha256(norm.encode("utf-8")).hexdigest()[:16]

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


def build_lookup_from_zip(zip_path: str) -> dict[str, str]:
    """
    Build a normalized-title→raw-title map from the zip's bat-file paths.

    Previously returned shortcodes; now returns the raw canonical title string
    so we can compute `thumbnail_key(raw_title)` at save time. The bat-stem
    title is the best we can recover from zip structure alone — it's what the
    LaunchBox app wrote as a filename-safe variant of the XML Title. If the
    same game is also present in the primary XML, the XML builder wins (richer
    punctuation; better for matching the hash we compute in generate_db.rs).
    """
    mapping: dict[str, str] = {}
    with zipfile.ZipFile(zip_path, "r") as zf:
        for name in zf.namelist():
            m = _BAT_RE.match(name)
            if m is None:
                continue
            _sc, bat_stem = m.group(1), m.group(2)
            if bat_stem.lower() in _SKIP_STEMS or "/Extras/" in name:
                continue
            title = _YEAR_SUFFIX.sub("", bat_stem).strip()
            if not title:
                continue
            for variant in title_variants(title):
                mapping.setdefault(normalize(prenormalize(variant)), title)
                sw = article_swap(variant)
                if sw:
                    mapping.setdefault(normalize(prenormalize(sw)), title)
    return mapping


def build_lookup_from_db(db_path: str) -> dict[str, str]:
    """
    Build a normalized-title→raw-title map from the pre-built exodium.db.
    DB wins over zip/XML because generate_db.rs uses the canonical XML title
    imported at build time — the exact string we want to hash.
    """
    mapping: dict[str, str] = {}
    conn = sqlite3.connect(db_path)
    try:
        for (title,) in conn.execute(
            "SELECT title FROM games WHERE title IS NOT NULL AND title != ''"
        ):
            for variant in title_variants(title):
                mapping.setdefault(normalize(prenormalize(variant)), title)
                sw = article_swap(variant)
                if sw:
                    mapping.setdefault(normalize(prenormalize(sw)), title)
    except sqlite3.OperationalError as e:
        print(f"  WARNING: could not read {db_path} ({e}). "
              f"Run 'cargo run --bin generate_db' to rebuild it.")
    finally:
        conn.close()
    return mapping


def build_lookup_from_xml(xml_gz_path: str) -> dict[str, str]:
    """
    Parse an XML catalogue and return a normalized-title→raw-title map.
    Both the canonical LaunchBox title ("Amt, Das") and the natural-order form
    ("Das Amt") are indexed so that image files using either convention match.
    """
    mapping: dict[str, str] = {}
    with gzip.open(xml_gz_path, "rb") as f:
        tree = ET.parse(f)

    for game in tree.findall(".//Game"):
        title_el = game.find("Title")
        if title_el is None:
            continue
        title = (title_el.text or "").strip()
        if not title:
            continue
        for variant in title_variants(title):
            mapping.setdefault(normalize(prenormalize(variant)), title)
            swapped = article_swap(variant)
            if swapped:
                mapping.setdefault(normalize(prenormalize(swapped)), title)

    return mapping


def lookup(title_to_raw: dict[str, str], title: str) -> str | None:
    """Try all title variants (full, subtitle-stripped) and article-swapped forms.
       Returns the raw title string to hash, or None if no match."""
    for variant in title_variants(title):
        raw = title_to_raw.get(normalize(prenormalize(variant)))
        if raw:
            return raw
        swapped = article_swap(variant)
        if swapped:
            raw = title_to_raw.get(normalize(prenormalize(swapped)))
            if raw:
                return raw
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
    # 1. DB titles (authoritative — generate_db.rs imported the canonical XML title)
    # 2. Zip bat structure (covers LP-exclusive games not in the primary XML)
    # 3. Primary XML catalogue
    # 4. Extra XML catalogue (fallback)

    title_lookup: dict[str, str] = {}

    if db_path:
        print(f"Loading titles from {db_path}...")
        title_lookup = build_lookup_from_db(db_path)
        print(f"  {len(title_lookup)} keys from DB (authoritative)")

    zip_map = build_lookup_from_zip(metadata_zip_path)
    before = len(title_lookup)
    for k, v in zip_map.items():
        title_lookup.setdefault(k, v)
    print(f"  +{len(title_lookup) - before} additional keys from zip structure")

    print(f"Parsing {xml_gz_path} for title lookup...")
    xml_map = build_lookup_from_xml(xml_gz_path)
    before = len(title_lookup)
    for k, v in xml_map.items():
        title_lookup.setdefault(k, v)
    print(f"  +{len(title_lookup) - before} additional keys from XML")

    if extra_xml:
        print(f"Merging extra catalogue {extra_xml}...")
        extra_map = build_lookup_from_xml(extra_xml)
        before = len(title_lookup)
        for k, v in extra_map.items():
            title_lookup.setdefault(k, v)
        print(f"  +{len(title_lookup) - before} additional keys")

    print(f"Opening {metadata_zip_path}...")
    with zipfile.ZipFile(metadata_zip_path, "r") as zf:
        # Primary: Box - Front images. Extensions include .gif because some
        # older eXoDOS entries (e.g. "3-D Pitfall") ship animated-era GIFs
        # as their only box-front asset; Pillow handles GIF → JPEG fine.
        allowed_ext = (".png", ".jpg", ".jpeg", ".gif", ".webp")
        box_front = [
            n for n in zf.namelist()
            if n.startswith("Images/MS-DOS/Box - Front/")
            and n.lower().endswith(allowed_ext)
        ]
        # Fallback layer: for titles with NO Box - Front entry at all, use
        # Fanart - Box - Front (e.g. "3-K Trivia" only ships fan-rendered
        # cover art, no official box scan). Index primary titles first so
        # fanart only fills genuine gaps and never overrides.
        primary_titles = {image_stem_to_title(n) for n in box_front}
        fanart = [
            n for n in zf.namelist()
            if n.startswith("Images/MS-DOS/Fanart - Box - Front/")
            and n.lower().endswith(allowed_ext)
            and image_stem_to_title(n) not in primary_titles
        ]
        box_front.extend(fanart)
        print(f"  {len(box_front) - len(fanart)} Box - Front + {len(fanart)} Fanart fallback images")

        matched = 0
        skipped = 0
        unmatched: list[str] = []

        for i, name in enumerate(box_front):
            if i > 0 and i % 500 == 0:
                print(f"  {i}/{len(box_front)}  matched={matched}  skipped={skipped}  unmatched={len(unmatched)}")

            stem_title = image_stem_to_title(name)
            raw_title = lookup(title_lookup, stem_title)

            if not raw_title:
                unmatched.append(name)
                continue

            # Content-addressed filename: SHA-256(normalized raw title)[:16].
            # Same hash function the Rust side uses to populate games.thumbnail_key
            # so both ends agree on the filename without any intermediate mapping.
            key = thumbnail_key(raw_title)
            out_path = output_dir / f"{key}.jpg"
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
                        preview_path = preview_dir / f"{key}.jpg"
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
        # Show a longer sample so the CI build log is useful for diagnosing
        # which specific LP titles fail title→shortcode lookup.
        sample = unmatched[:30]
        print(f"  First {len(sample)} unmatched titles (of {len(unmatched)}):")
        for n in sample:
            print(f"    {n}")


if __name__ == "__main__":
    main()
