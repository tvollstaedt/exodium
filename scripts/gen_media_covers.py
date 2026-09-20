#!/usr/bin/env python3
"""
Turn the Media Pack's cover art into the Lesesaal's `thumbnails/eXoMedia/` set.

The covers come out of the three archives by range-reading their `Images/`
region, which is contiguous (1,552 files, 604 MB, ~4 minutes over the swarm):

    EXTRACT_PREFIX="Images/MS-DOS Catalogs/Box - Front/" \\
    EXTRACT_OUT=~/.exodium-dev/mediapack/covers/catalog \\
    cargo run --example media_pack_spike -- <mediapack.torrent> <data> \\
        "Content/DOSCatalogs.zip" /tmp/listing.jsonl 600

LaunchBox names them `<Title>-01.jpg`, so the title - and with it the
content-addressed key the grid looks up - comes from the file name.

Usage:
    python3 scripts/gen_media_covers.py [<covers_dir>]
    (default: ~/.exodium-dev/mediapack/covers, or $XDO_DEV_DATA/mediapack/covers)

Then: python3 scripts/gen_previews.py eXoMedia

Dependencies: Pillow
"""

import gzip
import hashlib
import json
import os
import re
import sys
from pathlib import Path

from PIL import Image

REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "thumbnails" / "eXoMedia"
# Same as gen_thumbnails.py: 400 px Q90 is the source both the Tier 0 previews
# and a future poster pack are derived from.
WIDTH, QUALITY = 400, 90
SUFFIX = re.compile(r"-\d+$")


def thumbnail_key(title: str) -> str:
    norm = "".join(c for c in title.lower() if c.isascii() and c.isalnum())
    return hashlib.sha256(norm.encode("utf-8")).hexdigest()[:16]


def main() -> int:
    default = Path(os.environ.get("XDO_DEV_DATA", Path.home() / ".exodium-dev")) / "mediapack" / "covers"
    src = Path(sys.argv[1]) if len(sys.argv) > 1 else default
    if not src.is_dir():
        print(f"ERROR: no covers at {src}\n{__doc__}")
        return 1

    index = REPO / "metadata" / "media.json.gz"
    wanted: dict[str, str] = {}
    if index.exists():
        with gzip.open(index, "rt", encoding="utf-8") as fh:
            for issue in json.load(fh)["issues"]:
                if issue.get("cover_key"):
                    wanted[issue["cover_key"]] = issue["title"]

    OUT.mkdir(parents=True, exist_ok=True)
    written, unmatched = 0, 0
    seen: set[str] = set()
    for cover in sorted(src.rglob("*")):
        if cover.suffix.lower() not in (".jpg", ".jpeg", ".png", ".gif"):
            continue
        title = SUFFIX.sub("", cover.stem)
        key = thumbnail_key(title)
        if wanted and key not in wanted:
            unmatched += 1
            continue
        # LaunchBox ships several scans per title (`-01`, `-02`); the first one
        # is the front cover and the only one the grid shows.
        if key in seen:
            continue
        seen.add(key)
        image = Image.open(cover).convert("RGB")
        height = max(1, int(image.height * WIDTH / image.width))
        image.resize((WIDTH, height), Image.LANCZOS).save(
            OUT / f"{key}.jpg", "JPEG", quality=QUALITY, optimize=True
        )
        written += 1

    total = sum(p.stat().st_size for p in OUT.glob("*.jpg"))
    print(f"eXoMedia: {written} covers, {total / 1048576:.1f} MB")
    if wanted:
        missing = len(set(wanted) - seen)
        print(f"  {len(seen)}/{len(wanted)} issues covered, {missing} without art, {unmatched} files unmatched")
    return 0


if __name__ == "__main__":
    sys.exit(main())
