#!/usr/bin/env python3
"""
Rebuild the bundled eXoScummVM assets from the pack's own archives.

Both inputs come off the eXoScummVM torrent (see init-dev.sh --scummvm):
    <data>/eXoScummVM/eXoScummVM/Content/XOScummVMMetadata.zip  media + XML, 3.8 GB
    <data>/eXoScummVM/eXoScummVM/eXo/util/utilSVM.zip           launcher index +
                                                                 the seven ScummVM
                                                                 builds, 1.0 GB

Produces:
    metadata/ScummVM.xml.gz       the catalogue: xml/all/ScummVM.xml and
                                  xml/all/ScummVM SVN.xml merged into one
                                  LaunchBox document (both keep their own
                                  <Platform>, which is how a row knows it is
                                  one of eXo's "SVN - may be unstable" titles)
    metadata/scummvm.txt          eXo's launch index, verbatim (LF):
                                  <dir>;<engine:gameid | target>;<build>\\scummvm.exe
                                  The launcher looks a game up by the directory
                                  it runs from, exactly like launch_svm.bat does
    metadata/dosboxsvm.txt        <dir>:<build slug>\\dosbox.exe - the same shape
                                  as dosbox3x.txt so generate_db fills
                                  dosbox_variant with the pinned ScummVM build
    metadata/scummvm_ini/<slug>.ini
                                  each pinned build's scummvm.ini, verbatim. The
                                  [scummvm] section is eXo's defaults, the other
                                  sections are the targets 44 games launch by
                                  name instead of by engine:gameid

Usage:
    python3 scripts/gen_scummvm_assets.py [<pack_dir>]
    (default: ~/.exodium-dev/eXoScummVM/eXoScummVM, or $XDO_DEV_DATA)
"""

import gzip
import os
import re
import sys
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# eXo keeps every build under eXo/emulators/scmvm/<dir>/scummvm.exe; the
# root scummvm.exe is a build too (2.8.0, despite the ini's versioninfo).
# The slug is the dir name so the Windows resolver can join it verbatim.
ROOT_SLUG = "default"


def build_slug(exe_rel: str) -> str:
    """'2.9.0\\scummvm.exe' -> '2.9.0', 'scummvm.exe' -> ROOT_SLUG."""
    head, sep, _ = exe_rel.rpartition("\\")
    return head if sep else ROOT_SLUG


def bdecode(data: bytes, i: int = 0):
    """Minimal bencode reader - enough for a .torrent's file list."""
    c = data[i:i + 1]
    if c == b"i":
        end = data.index(b"e", i)
        return int(data[i + 1:end]), end + 1
    if c == b"l" or c == b"d":
        out, i = ([] if c == b"l" else {}), i + 1
        while data[i:i + 1] != b"e":
            if isinstance(out, list):
                v, i = bdecode(data, i)
                out.append(v)
            else:
                k, i = bdecode(data, i)
                v, i = bdecode(data, i)
                out[k.decode("utf-8", "replace")] = v
        return out, i + 1
    colon = data.index(b":", i)
    n = int(data[i:colon])
    return data[colon + 1:colon + 1 + n], colon + 1 + n


def torrent_zip_stems(torrent: Path) -> list[str]:
    """Stems of the game zips in the torrent (`eXo/eXoScummVM/<stem>.zip`)."""
    meta, _ = bdecode(torrent.read_bytes())
    stems = []
    for f in meta["info"]["files"]:
        parts = [p.decode("utf-8", "replace") for p in f["path"]]
        if len(parts) == 3 and parts[:2] == ["eXo", "eXoScummVM"] and parts[2].lower().endswith(".zip"):
            stems.append(parts[2][:-4])
    return stems


def align_dir_case(xml: bytes, stems: list[str]) -> tuple[bytes, int]:
    """Spell each game's `!ScummVM\\<dir>` the way its zip is spelled.

    Three catalogue rows differ from their zip only in case ("Escape from
    Hell" vs "Escape From Hell"). The zip's top-level folder follows the zip,
    the catalogue's directory becomes the shortcode, and installed-game
    matching compares the two exactly - on Linux the game would extract fine
    and still read as not installed."""
    by_fold = {s.casefold(): s for s in stems}
    fixed = 0

    def fix(m):
        nonlocal fixed
        name = m.group(2)
        want = by_fold.get(name.casefold())
        if want and want != name:
            fixed += 1
            return m.group(1) + want
        return m.group(0)

    text = xml.decode("utf-8")
    text = re.sub(r"(eXo\\eXoScummVM\\!ScummVM\\)([^\\<]+)", fix, text)
    return text.encode("utf-8"), fixed


def merge_launchbox_xml(docs: list[bytes]) -> bytes:
    """Merge several LaunchBox XML files into one document.

    All <Game> blocks come first, then everything else (<AlternateName>,
    <AdditionalApplication>): the importer's serde model reads each element
    kind as one contiguous run and rejects a <Game> that follows an
    <AlternateName> as a "duplicate field"."""
    head, games, others = None, [], []
    for raw in docs:
        text = raw.decode("utf-8")
        open_end = text.index("<LaunchBox>") + len("<LaunchBox>")
        close = text.rindex("</LaunchBox>")
        if head is None:
            head = text[:open_end]
        body = text[open_end:close]
        games.extend(re.findall(r"  <Game>.*?</Game>", body, re.S))
        others.append(re.sub(r"  <Game>.*?</Game>\n?", "", body, flags=re.S).strip("\n"))
    return (head + "\n" + "\n".join(games + [o for o in others if o]) + "\n</LaunchBox>").encode("utf-8")


def main() -> None:
    default = Path(os.environ.get("XDO_DEV_DATA", Path.home() / ".exodium-dev"))
    pack = Path(sys.argv[1]) if len(sys.argv) > 1 else default / "eXoScummVM/eXoScummVM"
    media_zip = pack / "Content/XOScummVMMetadata.zip"
    util_zip = pack / "eXo/util/utilSVM.zip"
    for p in (media_zip, util_zip):
        if not p.is_file():
            sys.exit(f"Missing {p}\nRun: pnpm run init-dev --scummvm")

    # Catalogue: two platforms, one document.
    with zipfile.ZipFile(media_zip) as zf:
        xml = merge_launchbox_xml([zf.read("xml/all/ScummVM.xml"), zf.read("xml/all/ScummVM SVN.xml")])
    xml, fixed = align_dir_case(xml, torrent_zip_stems(REPO / "torrents/eXoScummVM.torrent"))
    out = REPO / "metadata/ScummVM.xml.gz"
    out.write_bytes(gzip.compress(xml, 9))
    print(f"{out.name}: {xml.count(b'<Game>')} games ({fixed} directory names re-cased to match "
          f"their zip), {out.stat().st_size / 1048576:.1f} MB")

    with zipfile.ZipFile(util_zip) as zu:
        index = zu.read("scummvm.txt").decode("ascii").replace("\r\n", "\n").strip("\n") + "\n"
        # EXTsvm.zip is a 1 GB member; extract it once beside the outer zip so
        # the nested inis can be read with a seekable file (ZipExtFile
        # re-decompresses from the start on every backward seek).
        ext_path = util_zip.with_name("EXTsvm.zip")
        if not ext_path.is_file():
            print("Extracting EXTsvm.zip (1 GB, once)...")
            zu.extract("EXTsvm.zip", util_zip.parent)

    out = REPO / "metadata/scummvm.txt"
    out.write_text(index, encoding="utf-8")
    entries = [line.split(";") for line in index.splitlines() if line.strip()]
    bad = [e for e in entries if len(e) != 3]
    if bad:
        sys.exit(f"scummvm.txt: {len(bad)} malformed lines, e.g. {bad[0]}")
    print(f"{out.name}: {len(entries)} entries")

    lines = sorted(f"{name}:{build_slug(exe)}\\dosbox.exe" for name, _, exe in entries)
    out = REPO / "metadata/dosboxsvm.txt"
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")
    slugs = sorted({build_slug(exe) for _, _, exe in entries})
    print(f"{out.name}: {len(lines)} entries, builds: {', '.join(slugs)}")

    ini_dir = REPO / "metadata/scummvm_ini"
    ini_dir.mkdir(exist_ok=True)
    for stale in ini_dir.glob("*.ini"):
        stale.unlink()
    with zipfile.ZipFile(ext_path) as ze:
        by_slug = {}
        for name in ze.namelist():
            m = re.fullmatch(r"emulators/scmvm/(?:([^/]+)/)?scummvm\.ini", name)
            if m:
                by_slug[m.group(1) or ROOT_SLUG] = name
        missing = [s for s in slugs if s not in by_slug]
        if missing:
            sys.exit(f"EXTsvm.zip carries no scummvm.ini for builds: {missing}")
        for slug in slugs:
            ini = ze.read(by_slug[slug]).decode("utf-8", "replace").replace("\r\n", "\n")
            (ini_dir / f"{slug}.ini").write_text(ini, encoding="utf-8")
            targets = [s for s in re.findall(r"^\[([^\]]+)\]", ini, re.M) if s != "scummvm"]
            print(f"  scummvm_ini/{slug}.ini: {len(targets)} targets")


if __name__ == "__main__":
    main()
