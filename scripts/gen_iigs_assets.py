#!/usr/bin/env python3
"""
Rebuild the bundled eXoAppleIIGS assets from the pack's own archives.

All three inputs come off the eXoAppleIIGS torrent (see init-dev.sh --iigs):
    <pack>/Content/XOAppleIIGSMetadata.zip   catalogue XML + covers, 1.7 GB
    <pack>/Content/!AppleIIGSmetadata.zip    per-game launch bats, emulator
                                             configs and Extras PDFs, 2.1 GB
    <pack>/eXo/util/utilAppleIIGS.zip        eXo's emulator index, 148 MB

Produces:
    metadata/AppleIIGS.xml.gz       the catalogue (xml/all: Exodium does not
                                    curate), <Game> blocks first
    metadata/AppleIIGS_configs.zip  every file of !appleiigs/<dir>/ except the
                                    Extras, under eXo/eXoAppleIIGS/ like eXo's
                                    own tree
    metadata/appleiigs.json         the launch index. eXo decides per game with
                                    a batch file (exception.bat) that sets disks,
                                    speed and the config, sometimes behind a
                                    console menu; this is that decision as data,
                                    so the launcher never interprets a bat.
                                    scripts/iigs_playtest.tsv adds how each MAME
                                    and GSport game fared under GSplus 1.38

Usage:
    python3 scripts/gen_iigs_assets.py [<pack_dir>]
    (default: ~/.exodium-dev/eXoAppleIIGS/eXoAppleIIGS, or $XDO_DEV_DATA)
"""

import collections
import gzip
import json
import os
import re
import sys
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PLAYTEST = REPO / "scripts/iigs_playtest.tsv"
PREFIX = "eXo/eXoAppleIIGS/!appleiigs/"
ENGINES = {"gsplus\\gsplus.exe": "gsplus", "gsport\\gsport.exe": "gsport", "mame": "mame"}
# Lines of an exception.bat that only steer eXo's console window.
NOISE = re.compile(
    r"^(@?echo (off|on)|echo\.|cls|pause|cd |goto |if errorlevel|:|"
    r"\.\.\\\.\.\\\.\.\\util\\(setconsole|choice)|\.\\util\\capslock)",
    re.I,
)
PRESS = re.compile(r"^Press (\d) (?:to play |to use |for |to )?(.+)$", re.I)
# LaunchBox's folder for printer output; Exodium says where it goes itself.
LAUNCHBOX_PATH = re.compile(r"^(Printer output is saved in the folder:|\\Launchbox\\)", re.I)


def merge_launchbox_xml(raw: bytes) -> bytes:
    """All <Game> blocks first: the importer reads each element kind as one
    contiguous run and rejects a <Game> after an <AdditionalApplication>."""
    text = raw.decode("utf-8-sig")
    open_end = text.index("<LaunchBox>") + len("<LaunchBox>")
    close = text.rindex("</LaunchBox>")
    body = text[open_end:close]
    games = re.findall(r"  <Game>.*?</Game>", body, re.S)
    others = re.sub(r"  <Game>.*?</Game>\n?", "", body, flags=re.S).strip("\n")
    parts = games + ([others] if others else [])
    return (text[:open_end] + "\n" + "\n".join(parts) + "\n</LaunchBox>\n").encode("utf-8")


def bat_lines(raw: bytes) -> list[str]:
    return [l.strip() for l in raw.decode("cp437").replace("\r", "").split("\n")]


def paragraphs(lines: list[str]) -> str | None:
    """echo lines as prose: eXo wraps at console width, `echo.` breaks, and a
    line opening like the one before it is a list item ("Add-On 1 is ...",
    "Add-On 2 is ...") rather than a wrapped sentence."""
    out, cur = [], []
    for line in lines:
        if line is None:
            if cur:
                out.append(" ".join(cur))
            cur = []
        elif cur and line.split()[:1] == cur[-1].split()[:1]:
            out.append(" ".join(cur))
            cur = [line]
        else:
            cur.append(line)
    if cur:
        out.append(" ".join(cur))
    text = "\n".join(p for p in out if p)
    return text or None


def parse_exception(raw: bytes) -> dict:
    """What an exception.bat decides. Top-level SETs are the game's own;
    a `choice` menu yields one option per answer, each with its own SETs and
    echo lines, labelled from eXo's "Press N ..." prompt."""
    lines = bat_lines(raw)
    sections: dict[str | None, list[str]] = {None: []}
    current = None
    for line in lines:
        m = re.match(r"^:(\w+)$", line)
        if m:
            current = m.group(1).lower()
            sections.setdefault(current, [])
            continue
        sections[current].append(line)

    def settings(body: list[str]) -> dict:
        out = {}
        for line in body:
            m = re.match(r"^set (\w+)=(.*)$", line, re.I)
            if m:
                out[m.group(1).upper()] = m.group(2).strip()
        return out

    def notes(body: list[str], skip_press: bool) -> str | None:
        kept: list[str | None] = []
        for line in body:
            m = re.match(r"^echo(?: (.*))?$", line, re.I)
            if line.lower() == "echo." or (m and not (m.group(1) or "").strip()):
                kept.append(None)
                continue
            if not m or NOISE.match(line):
                continue
            text = m.group(1).strip()
            if text.lower() in ("off", "on"):
                continue
            if skip_press and PRESS.match(text):
                continue
            if LAUNCHBOX_PATH.match(text):
                continue
            kept.append(text)
        return paragraphs(kept)

    top = sections[None]
    result = {
        "set": settings(top),
        "note": notes(top, True),
        "capslock": any(re.search(r"capslock\.exe on", l, re.I) for l in lines),
        "options": [],
    }
    choice = next((l for l in top if re.search(r"\\choice /C:", l, re.I)), None)
    if choice is None:
        return result
    keys = re.search(r"/C:(\w+)", choice, re.I).group(1)
    targets = {}
    for line in top:
        m = re.match(r"^if errorlevel\s*=?\s*(\d+) goto (\w+)$", line, re.I)
        if m:
            targets[int(m.group(1))] = m.group(2).lower()
    labels = {}
    for line in top:
        m = re.match(r"^echo (.*)$", line, re.I)
        p = PRESS.match(m.group(1).strip()) if m else None
        if p:
            labels[int(p.group(1))] = p.group(2).strip()
    for n in range(1, len(keys) + 1):
        body = sections.get(targets.get(n, ""), [])
        result["options"].append({
            "label": labels.get(n, f"Option {n}"),
            "set": settings(body),
            "note": notes(body, False),
        })
    return result


def short_labels(labels: list[str]) -> list[str]:
    """Drop the part every label shares up to a ": " or " - " boundary:
    "Designasaurus: Walk a Dinosaur" -> "Walk a Dinosaur"."""
    if len(labels) < 2:
        return labels
    prefix = os.path.commonprefix(labels)
    cut = max(prefix.rfind(": ") + 2 if ": " in prefix else 0, prefix.rfind(" - ") + 3 if " - " in prefix else 0)
    if cut <= 0 or any(len(l) <= cut for l in labels):
        return labels
    return [l[cut:] for l in labels]


def disks_of(s: dict) -> list[str] | None:
    """eXo's DISK1..DISKn, in the order MAME mounts them."""
    if "DISK1" not in s:
        return None
    n = int(s.get("DISKS", "1") or "1")
    disks = [s.get(f"DISK{i}", "") for i in range(1, n + 1)]
    if any(not d for d in disks):
        raise ValueError(f"DISKS={n} but {disks}")
    return disks


def config_keys(raw: bytes) -> dict[str, str]:
    out = {}
    for line in raw.decode("latin-1").replace("\r", "").split("\n"):
        m = re.match(r"^(\w+(?:\[[0-9a-f]+\])?)\s*=\s*(.*)$", line)
        if m:
            out[m.group(1)] = m.group(2).strip()
    return out


def main() -> None:
    default = Path(os.environ.get("XDO_DEV_DATA", Path.home() / ".exodium-dev"))
    pack = Path(sys.argv[1]) if len(sys.argv) > 1 else default / "eXoAppleIIGS/eXoAppleIIGS"
    media_zip = pack / "Content/XOAppleIIGSMetadata.zip"
    bang_zip = pack / "Content/!AppleIIGSmetadata.zip"
    util_zip = pack / "eXo/util/utilAppleIIGS.zip"
    for p in (media_zip, bang_zip, util_zip):
        if not p.is_file():
            sys.exit(f"Missing {p}\nRun: pnpm run init-dev --iigs")

    with zipfile.ZipFile(media_zip) as zf:
        xml = merge_launchbox_xml(zf.read("xml/all/Apple IIGS.xml"))
    out = REPO / "metadata/AppleIIGS.xml.gz"
    out.write_bytes(gzip.compress(xml, 9))
    print(f"{out.name}: {xml.count(b'<Game>')} rows, {out.stat().st_size / 1048576:.1f} MB")

    with zipfile.ZipFile(util_zip) as zu:
        index_text = zu.read("appleIIGS.txt").decode("ascii")
    engines = {}
    for line in index_text.replace("\r", "").split("\n"):
        if not line.strip():
            continue
        title, _, exe = line.rpartition(":")
        engine = ENGINES.get(exe.strip().lower())
        if engine is None:
            sys.exit(f"appleIIGS.txt: unknown emulator in {line!r}")
        engines[title] = engine

    games: dict[str, dict] = {}
    brams = collections.Counter()
    config_out = REPO / "metadata/AppleIIGS_configs.zip"
    with zipfile.ZipFile(bang_zip) as zb, zipfile.ZipFile(config_out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as zc:
        files: dict[str, dict[str, zipfile.ZipInfo]] = collections.defaultdict(dict)
        extras: dict[str, list] = collections.defaultdict(list)
        for info in zb.infolist():
            if info.is_dir() or not info.filename.startswith(PREFIX):
                continue
            rest = info.filename[len(PREFIX):]
            gdir, _, name = rest.partition("/")
            if name.startswith("Extras/"):
                if not name.lower().endswith(".bat"):
                    extras[gdir].append({"name": name[len("Extras/"):], "size": info.file_size})
                continue
            if "/" in name:
                sys.exit(f"unexpected nested file {info.filename}")
            files[gdir][name] = info
            # A fresh ZipInfo: writestr records the OUTPUT offset in the one it
            # is given, and `info` still has to read from the input.
            out_info = zipfile.ZipInfo(info.filename, info.date_time)
            out_info.compress_type = zipfile.ZIP_DEFLATED
            zc.writestr(out_info, zb.read(info))

        for gdir in sorted(files):
            if gdir not in engines:
                sys.exit(f"{gdir} has no line in appleIIGS.txt")
            entry: dict = {"engine": engines[gdir]}
            by_lower = {n.lower(): i for n, i in files[gdir].items()}
            exc = parse_exception(zb.read(by_lower["exception.bat"])) if "exception.bat" in by_lower else None
            base = exc["set"] if exc else {}
            try:
                disks = disks_of(base)
            except ValueError as e:
                sys.exit(f"{gdir}: {e}")
            if disks:
                entry["disks"] = disks
            if base.get("SPEED", "").upper() == "TRUE":
                entry["fast"] = True
            if base.get("CONFIGTXT"):
                entry["config"] = base["CONFIGTXT"]
            if exc and exc["note"]:
                entry["note"] = exc["note"]
            if exc and exc["capslock"]:
                entry["capslock"] = True
            if exc and exc["options"]:
                opts = []
                for opt, label in zip(exc["options"], short_labels([o["label"] for o in exc["options"]])):
                    o: dict = {"label": label}
                    s = opt["set"]
                    if s.get("CONFIGTXT"):
                        o["config"] = s["CONFIGTXT"]
                    try:
                        d = disks_of(s)
                    except ValueError as e:
                        sys.exit(f"{gdir} option {label}: {e}")
                    if d:
                        o["disks"] = d
                    if s.get("SPEED", "").upper() == "TRUE":
                        o["fast"] = True
                    if opt["note"]:
                        o["note"] = opt["note"]
                    if not (o.get("config") or o.get("disks")):
                        sys.exit(f"{gdir} option {label!r} selects nothing")
                    opts.append(o)
                entry["options"] = opts

            configs = [n for n in files[gdir] if n.lower().endswith(".txt")]
            for name in configs:
                keys = config_keys(zb.read(files[gdir][name]))
                bram = tuple(sorted((k, v) for k, v in keys.items() if k.startswith("bram")))
                if bram:
                    brams[bram] += 1
                if keys.get("g_parallel") == "1":
                    entry["printing"] = True
            wanted = {o.get("config") for o in entry.get("options", [])} | {entry.get("config")}
            for name in wanted - {None}:
                if name.lower() not in by_lower:
                    sys.exit(f"{gdir}: config {name} is not in the game's folder")

            if entry["engine"] == "mame":
                if not entry.get("disks") and not any(o.get("disks") for o in entry.get("options", [])):
                    sys.exit(f"{gdir}: a MAME game without DISK1 cannot launch")
            elif entry["engine"] == "gsplus" and not configs:
                sys.exit(f"{gdir}: GSplus game without a config")
            if extras.get(gdir):
                entry["extras"] = sorted(extras[gdir], key=lambda e: e["name"].lower())
            games[gdir] = entry

    missing = sorted(set(engines) - set(games))
    if missing:
        sys.exit(f"appleIIGS.txt names games without a folder: {missing[:5]}")

    # How eXo's MAME and GSport games fare under GSplus 1.38, which runs them
    # on macOS and Linux. Absent means untested.
    for line in PLAYTEST.read_text(encoding="utf-8").splitlines()[1:]:
        if not line.strip():
            continue
        gdir, status, *note = line.split("\t")
        if gdir not in games:
            sys.exit(f"{PLAYTEST.name}: unknown game {gdir!r}")
        if status not in ("ok", "issues", "broken"):
            sys.exit(f"{PLAYTEST.name}: {gdir}: status must be ok, issues or broken, not {status!r}")
        if games[gdir]["engine"] == "gsplus":
            sys.exit(f"{PLAYTEST.name}: {gdir} already runs under GSplus at eXo")
        games[gdir]["gsplus"] = status
        if note and note[0].strip():
            games[gdir]["gsplus_note"] = note[0].strip()
    print(f"{config_out.name}: {sum(len(f) for f in files.values())} files, "
          f"{config_out.stat().st_size / 1024:.0f} KB")

    # Games without a config (MAME's) get eXo's most common control-panel
    # BRAM when they run under GSplus instead.
    default_bram = [f"{k} = {v}" for k, v in brams.most_common(1)[0][0]]
    index = {"schema": 1, "default_bram": default_bram, "games": games}
    out = REPO / "metadata/appleiigs.json"
    out.write_text(json.dumps(index, indent=1, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")
    by_engine = collections.Counter(g["engine"] for g in games.values())
    print(f"{out.name}: {len(games)} games ({dict(by_engine)}), "
          f"{sum(1 for g in games.values() if g.get('options'))} with a menu, "
          f"{sum(1 for g in games.values() if g.get('note'))} with a note, "
          f"{sum(1 for g in games.values() if g.get('extras'))} with extras")


if __name__ == "__main__":
    main()
