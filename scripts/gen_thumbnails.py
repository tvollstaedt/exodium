#!/usr/bin/env python3
"""
Generate shortcode-keyed thumbnail JPEGs from an eXoDOS metadata ZIP.

Works for the main eXoDOS pack and all language packs (GLP/SLP/PLP).
Each pack uses platform 'MS-DOS' so image paths are always
Images/MS-DOS/Box - Front/<title>-NN.jpg inside the zip.

Usage:
    python3 scripts/gen_thumbnails.py <metadata_zip> <xml_gz> <output_dir> [--force]

    metadata_zip  Path to XODOSMetadata.zip / eXoDOS_GLP_Metadata.zip / etc.
    xml_gz        Matching bundled catalogue: metadata/MS-DOS.xml.gz, GLP.xml.gz, etc.
    output_dir    Destination for shortcode-keyed JPEGs (e.g. thumbnails/eXoDOS)
    --force       Overwrite already-existing thumbnails

Dependencies: Pillow  (pip3 install Pillow)
"""

import gzip
import io
import re
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


def normalize(text: str) -> str:
    """Lowercase, strip accents, remove non-alphanumeric chars for fuzzy matching."""
    text = unicodedata.normalize("NFD", text)
    text = "".join(c for c in text if unicodedata.category(c) != "Mn")
    return re.sub(r"[^a-z0-9]", "", text.lower())


def build_title_to_shortcode(xml_gz_path: str) -> dict[str, str]:
    """
    Parse MS-DOS.xml.gz and return a map of normalized_title → shortcode.
    Shortcode is extracted from ApplicationPath: eXo\\eXoDOS\\<SC>\\dosbox.conf
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
            mapping[normalize(title)] = shortcode

    return mapping


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
    args = [a for a in sys.argv[1:] if not a.startswith("-")]
    force = "--force" in sys.argv

    if len(args) != 3:
        print(__doc__)
        sys.exit(1)

    metadata_zip_path, xml_gz_path, output_dir_str = args
    output_dir = Path(output_dir_str)
    output_dir.mkdir(parents=True, exist_ok=True)

    print(f"Parsing {xml_gz_path} for title→shortcode mapping...")
    title_to_sc = build_title_to_shortcode(xml_gz_path)
    print(f"  {len(title_to_sc)} games indexed")

    print(f"Opening {metadata_zip_path}...")
    with zipfile.ZipFile(metadata_zip_path, "r") as zf:
        box_front = [
            n for n in zf.namelist()
            if "Box - Front" in n and n.lower().endswith((".png", ".jpg", ".jpeg"))
        ]
        print(f"  {len(box_front)} box-front images found")

        matched = 0
        skipped = 0
        unmatched: list[str] = []

        for i, name in enumerate(box_front):
            if i > 0 and i % 500 == 0:
                print(f"  {i}/{len(box_front)}  matched={matched}  skipped={skipped}  unmatched={len(unmatched)}")

            title = image_stem_to_title(name)
            norm = normalize(title)
            shortcode = title_to_sc.get(norm)

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
                except Exception as e:
                    print(f"  WARN: failed to process {name}: {e}")

    total = matched + skipped
    print(f"\nDone: {total} thumbnails in {output_dir}")
    print(f"  New: {matched}  Already existed: {skipped}  Unmatched: {len(unmatched)}")
    if unmatched:
        print(f"  First 10 unmatched: {unmatched[:10]}")


if __name__ == "__main__":
    main()
