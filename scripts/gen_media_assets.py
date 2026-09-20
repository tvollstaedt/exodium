#!/usr/bin/env python3
"""
Rebuild the bundled eXoDOS Media Pack index from the pack's own archives.

Everything comes off the Media Pack torrent, read WITHOUT downloading the
archives (each is 1.7-102 GB):

    cargo run --example media_pack_spike -- <mediapack.torrent> <data> \\
        "Content/DOSMagazines.zip" <work>/DOSMagazines.jsonl 900
    EXTRACT="Data/Platforms/MS-DOS Magazines & Newsletters.xml" cargo run ... \\
        (writes <work>/DOSMagazines_x.extract -> rename to DOSMagazines.xml)

Work dir (default ~/.exodium-dev/mediapack, or $XDO_DEV_DATA/mediapack):
    DOSMagazines.xml / DOSBooks.xml / DOSCatalogs.xml     the platform catalogues
    DOSMagazines.jsonl / DOSBooks.jsonl / DOSCatalogs.jsonl   central directories
    DOS_linux_Magazines.zip                              2.8 MB, fetched whole

Produces:
    metadata/media.json.gz        one record per issue: publication, title,
                                  year, the entry inside the zip, and for the
                                  241 runnable disk magazines the launcher dir
                                  plus eXo's own command line. No zip offsets -
                                  the reader has the live central directory
                                  anyway and eXo may repack.
    metadata/media_articles.txt   <shortcode>;<kind>;<page>;<pdf entry>
                                  eXo's own per-game article index, read out of
                                  the Linux launcher scripts (which name the PDF
                                  and the page verbatim).

Usage:
    python3 scripts/gen_media_assets.py [<work_dir>]
"""

import gzip
import hashlib
import json
import os
import re
import sys
import zipfile
from pathlib import Path
from xml.etree import ElementTree as ET

ZIPS = {
    "magazine": ("DOSMagazines", "Content/DOSMagazines.zip"),
    "book": ("DOSBooks", "Content/DOSBooks.zip"),
    "catalog": ("DOSCatalogs", "Content/DOSCatalogs.zip"),
}
READABLE = (".pdf", ".jpg", ".jpeg", ".png")
# eXo's own typos in the launcher paths; the catalogue spells them differently.
SHORTCODE_FIXES = {"caeser2": "caesar2"}


def fix_shortcode(code: str) -> str:
    return SHORTCODE_FIXES.get(code, code)


def thumbnail_key(title: str) -> str:
    """SHA-256(alnum-only lowercase title)[:16], as in generate_db.rs."""
    norm = "".join(c for c in title.lower() if c.isascii() and c.isalnum())
    return hashlib.sha256(norm.encode("utf-8")).hexdigest()[:16]


def text(game, tag: str) -> str:
    el = game.find(tag)
    return (el.text or "").strip() if el is not None and el.text else ""


def zip_path(app_path: str) -> str:
    return app_path.replace("\\", "/").lstrip("./")


def publication_of(kind: str, genre: str) -> str:
    """
    eXo files the series in Genre: "Magazine / PC World", "Books / Programming",
    "Software Catalog". Multi-genre rows ("A; B") keep the first.
    """
    first = genre.split(";")[0].strip()
    if "/" in first:
        return first.split("/", 1)[1].strip()
    return first or {"magazine": "Magazines", "book": "Books", "catalog": "Catalogs"}[kind]


def series_dir_from_script(zf: zipfile.ZipFile, bat: str) -> str | None:
    """
    The three series launchers sit at eXo/Magazines/<Series>.bat and take the
    issue number as their argument; the directory they cd into is named only
    inside the script. The Linux twin states it as "./<dir>/run.bat".
    """
    name = "eXo/Magazines/" + Path(zip_path(bat)).stem + ".bsh"
    try:
        body = zf.read(name).decode("utf-8", "replace")
    except KeyError:
        return None
    m = re.search(r"\./([^/\n\"]+)/run\.(?:bat|bak)", body)
    return f"eXo/Magazines/{m.group(1)}" if m else None


# eXo's launcher substitutes placeholders in run.bat with sed before starting
# DOSBox, some of them behind a numeric test on the issue number.
SED_EXPR = re.compile(r'-e "s\|([^|]+)\|([^|]*)\|g"')
PARAMETER_ONE = "${parameterone}"
SED_CONDITION = re.compile(
    r'^\[\s*"?\$\{parameterone\}"?\s+(-eq|-ne|-lt|-le|-gt|-ge)\s+(\d+)\s*\]\s*&&'
)
COMPARE = {
    "-eq": lambda a, b: a == b,
    "-ne": lambda a, b: a != b,
    "-lt": lambda a, b: a < b,
    "-le": lambda a, b: a <= b,
    "-gt": lambda a, b: a > b,
    "-ge": lambda a, b: a >= b,
}


def substitutions_from_script(zf: zipfile.ZipFile, bat: str, issue: str | None) -> dict[str, str]:
    """
    What eXo's launcher writes into run.bat before starting DOSBox, for THIS
    issue. The rules and their thresholds are eXo's data and are read out of
    the Linux twin of the launcher; the restore pass (whose pattern is the
    issue number, not a placeholder) is not one of them.
    """
    name = Path(zip_path(bat)).with_suffix(".bsh").as_posix()
    try:
        body = zf.read(name).decode("utf-8", "replace")
    except KeyError:
        return {}

    out: dict[str, str] = {}
    for line in body.splitlines():
        line = line.strip()
        if "sed -i" not in line:
            continue
        guard = SED_CONDITION.match(line)
        if guard:
            if issue is None or not issue.lstrip("-").isdigit():
                continue
            if not COMPARE[guard.group(1)](int(issue, 10), int(guard.group(2), 10)):
                continue
        for rule in SED_EXPR.finditer(line):
            pattern, value = rule.group(1), rule.group(2)
            if "${" in pattern:
                continue
            if PARAMETER_ONE in value:
                if issue is None:
                    continue
                value = value.replace(PARAMETER_ONE, issue)
            # sed runs its expressions in order against text an earlier one
            # may already have rewritten, so the first rule for a key wins.
            out.setdefault(pattern, value)
    return out


def load_listing(path: Path) -> dict[str, int]:
    sizes = {}
    with path.open() as fh:
        for line in fh:
            row = json.loads(line)
            if not row["name"].endswith("/"):
                sizes[row["name"]] = row["uncompressed"]
    return sizes


def subtree_size(sizes: dict[str, int], prefix: str) -> int:
    prefix = prefix.rstrip("/") + "/"
    return sum(v for k, v in sizes.items() if k.startswith(prefix))


def loose_files(sizes: dict[str, int], directory: str) -> list[str]:
    """Entries sitting directly in a directory - the series' conf and launcher."""
    prefix = directory.rstrip("/") + "/"
    return [k for k in sizes if k.startswith(prefix) and "/" not in k[len(prefix):]]


def issue_dir_for(sizes: dict[str, int], launch_dir: str, command: str | None) -> str | None:
    """
    The subdirectory ONE issue of a series lives in. eXo selects it with the
    launcher's argument, which is the directory's name (`BBD/004`) or its
    suffix (`GameBytes/gbytes14`). Without this every issue of a series would
    claim the whole series: 199 MB per Big Blue Disk issue, 15.6 GB per
    Interactive Entertainment CD issue.
    """
    if not command:
        return None
    prefix = launch_dir.rstrip("/") + "/"
    subdirs = {k[len(prefix):].split("/")[0] for k in sizes if k.startswith(prefix) and "/" in k[len(prefix):]}
    if command in subdirs:
        return f"{prefix}{command}"
    matches = [d for d in subdirs if d.endswith(command)]
    return f"{prefix}{matches[0]}" if len(matches) == 1 else None


def collect_articles(zf: zipfile.ZipFile) -> list[str]:
    """
    eXo drops one launcher per magazine mention into the GAME's directory
    (!dos/<shortcode>/Magazines/<Kind> <Magazine> <date> page <N>.bsh) and has
    it open SumatraPDF on that page. Both the target and the page are read out
    of the script, so nothing here is guessed from the file name.
    """
    pat = re.compile(r"!dos/([^/]+)/Magazines/(\w+) .+\.bsh$")
    target = re.compile(r"-page (\d+) \"\.\./\.\./(Magazines/[^\"]+)\"")
    out = []
    for name in zf.namelist():
        m = pat.search(name)
        if not m:
            continue
        hit = target.search(zf.read(name).decode("utf-8", "replace"))
        if not hit:
            continue
        # eXo files two links on page 0; the reader counts from 1.
        page, rel = max(1, int(hit.group(1))), hit.group(2)
        out.append(f"{fix_shortcode(m.group(1))};{m.group(2)};{page};eXo/{rel}")
    return sorted(set(out))


GLP_ZIP = "Content/eXoDOS_GLP_Addonpack_MagazinesGLP.zip"
GLP_INNER = "Content/eXoDOS_GLP_Addonpack_MagazinesGLP_1.0.zip"


def pdf_for_title(pdfs: dict[str, str], title: str) -> str | None:
    """
    The fragment's title names the PDF - except that eXo zero-pads the special
    issues' numbers in the file name and not in the title ("ASM Sonderheft 1"
    vs "ASM Sonderheft 01.pdf").
    """
    if f"{title}.pdf" in pdfs:
        return pdfs[f"{title}.pdf"]
    padded = re.sub(r" (\d)$", r" 0\1", title)
    return pdfs.get(f"{padded}.pdf")


def collect_glp(glp: Path) -> tuple[list[dict] | None, list[str]]:
    """
    The German add-on inside the GLP torrent: one STORED archive in eXo's
    installer zip, 654 LaunchBox <Game> fragments as the catalogue (bare
    elements, no XML root), per-game launcher bats as the article index.
    Inputs come from media_pack_spike with NESTED_EXTRACT_PREFIX.
    """
    listing = glp / "GLPMag.nested.jsonl"
    if not listing.exists():
        print(f"ERROR: {listing} missing")
        return None, []
    sizes = load_listing(listing)
    pdfs = {Path(k).name: k for k in sizes if k.lower().endswith(".pdf")}
    extras: dict[str, int] = {}
    for name in sizes:
        m = re.match(r"eXo/Magazines/!mags/!german/([^/]+)/Extras/", name)
        if m:
            extras[m.group(1)] = extras.get(m.group(1), 0) + 1

    issues: list[dict] = []
    frag_dir = glp / "xml" / "!german" / "magazines"
    missing = 0
    for frag in sorted(p for p in frag_dir.iterdir() if p.is_file()):
        game = ET.fromstring(frag.read_text(encoding="utf-8"))
        title = text(game, "Title")
        entry = pdf_for_title(pdfs, title)
        if entry is None:
            print(f"  WARNING: no PDF for {title!r}, skipped")
            missing += 1
            continue
        release = text(game, "ReleaseDate")
        # The file stem is the canonical title - it is what the cover and the
        # article links are keyed on.
        stem = Path(entry).stem
        issues.append({
            "key": f"mag:{entry}",
            "kind": "magazine",
            "publication": publication_of("magazine", text(game, "Genre")),
            "title": stem,
            "sort_title": text(game, "SortTitle") or stem,
            "year": int(release[:4]) if release[:4].isdigit() else None,
            "release_date": release or None,
            "publisher": text(game, "Publisher") or None,
            "developer": text(game, "Developer") or None,
            "notes": text(game, "Notes") or None,
            "zip": GLP_ZIP,
            "entry": entry,
            "entry_kind": "pdf",
            "size": sizes[entry],
            "cover_key": thumbnail_key(stem),
            "runnable": False,
            "launch_dir": None,
            "issue_dir": None,
            "launch_bat": None,
            "command_line": None,
            "substitutions": {},
            "source": "eXoDOS_GLP",
            "inner_zip": GLP_INNER,
            "language": "DE",
            "extras_count": extras.get(stem, 0),
        })
    print(f"GLP magazines: {len(issues)} readable, {missing} without a PDF")
    if missing:
        return None, []

    # The bats name eXo's INSTALLED path (Magazines\!german\<S>\<H>.pdf); the
    # archive files the same PDF under eXo/Magazines/!german/.
    target = re.compile(r'-page (\d+) "\.\.\\\.\.\\Magazines\\!german\\([^"]+)"')
    articles: list[str] = []
    unresolved = 0
    for bat in sorted((glp / "eXo" / "eXoDOS" / "!dos" / "!german").rglob("*.bat")):
        m = re.search(r"!german/([^/]+)/Magazines/(\w+) ", bat.as_posix())
        hit = target.search(bat.read_text(encoding="utf-8", errors="replace"))
        if not m or not hit:
            unresolved += 1
            continue
        entry = pdf_for_title(pdfs, Path(hit.group(2).replace("\\", "/")).stem)
        if entry is None:
            unresolved += 1
            continue
        page = max(1, int(hit.group(1)))
        articles.append(f"{fix_shortcode(m.group(1))};{m.group(2)};{page};{entry}")
    articles = sorted(set(articles))
    print(f"GLP articles: {len(articles)} resolved, {unresolved} unresolved")
    if unresolved:
        return None, []
    return issues, articles


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    default = Path(os.environ.get("XDO_DEV_DATA", Path.home() / ".exodium-dev")) / "mediapack"
    work = Path(sys.argv[1]) if len(sys.argv) > 1 else default
    if not work.is_dir():
        print(f"ERROR: work dir not found: {work}\n{__doc__}")
        return 1

    linux_zip = work / "DOS_linux_Magazines.zip"
    if not linux_zip.exists():
        print(f"ERROR: {linux_zip} missing - it carries the launcher scripts")
        return 1
    helper = zipfile.ZipFile(linux_zip)

    issues = []
    for kind, (stem, zip_name) in ZIPS.items():
        xml_path, listing_path = work / f"{stem}.xml", work / f"{stem}.jsonl"
        for p in (xml_path, listing_path):
            if not p.exists():
                print(f"ERROR: {p} missing")
                return 1
        sizes = load_listing(listing_path)
        games = ET.parse(xml_path).getroot().findall("Game")
        readable = runnable = skipped = 0

        for game in games:
            app = zip_path(text(game, "ApplicationPath"))
            if not app:
                skipped += 1
                continue
            title = text(game, "Title")
            release = text(game, "ReleaseDate")
            record = {
                "key": "",
                "kind": kind,
                "publication": publication_of(kind, text(game, "Genre")),
                "title": title,
                "sort_title": text(game, "SortTitle") or title,
                "year": int(release[:4]) if release[:4].isdigit() else None,
                "release_date": release or None,
                "publisher": text(game, "Publisher") or None,
                "developer": text(game, "Developer") or None,
                "notes": text(game, "Notes") or None,
                "zip": zip_name,
                "entry": None,
                "entry_kind": None,
                "size": 0,
                "cover_key": thumbnail_key(title),
                "runnable": False,
                "launch_dir": None,
                "issue_dir": None,
                "launch_bat": None,
                "command_line": None,
                "substitutions": {},
                "source": "eXoMedia",
                "inner_zip": None,
                "language": "EN",
                "extras_count": 0,
            }

            if app.lower().endswith(READABLE):
                if app not in sizes:
                    print(f"  WARNING: {app} is not in the archive, skipped")
                    skipped += 1
                    continue
                record["entry"] = app
                record["entry_kind"] = "pdf" if app.lower().endswith(".pdf") else "image"
                record["size"] = sizes[app]
                record["key"] = f"{kind[:3]}:{app}"
                readable += 1
            else:
                command = text(game, "CommandLine") or None
                launch_dir = Path(app).parent.as_posix()
                if launch_dir == "eXo/Magazines":
                    launch_dir = series_dir_from_script(helper, app)
                    if launch_dir is None:
                        print(f"  WARNING: no launcher dir for {app}, skipped")
                        skipped += 1
                        continue
                # PC Gamer's launcher already stands in the issue's own
                # directory; a series launcher picks one by argument.
                own_dir = (
                    launch_dir
                    if Path(app).parent.as_posix() != "eXo/Magazines"
                    else issue_dir_for(sizes, launch_dir, command)
                )
                # What a launch needs: the issue plus the series' own conf and
                # launcher, which sit one level up.
                size = subtree_size(sizes, own_dir or launch_dir)
                if own_dir and own_dir != launch_dir:
                    size += sum(sizes[f] for f in loose_files(sizes, launch_dir))
                record.update(
                    runnable=True,
                    launch_dir=launch_dir,
                    issue_dir=own_dir,
                    launch_bat=Path(app).name,
                    command_line=command,
                    substitutions=substitutions_from_script(helper, app, command),
                    size=size,
                    key=f"{kind[:3]}:{launch_dir}#{command or Path(app).stem}",
                )
                runnable += 1
            issues.append(record)

        print(f"{stem}: {readable} readable, {runnable} runnable, {skipped} skipped")

    glp_articles: list[str] = []
    glp_dir = work / "glp"
    if glp_dir.is_dir():
        glp_issues, glp_articles = collect_glp(glp_dir)
        if glp_issues is None:
            return 1
        issues.extend(glp_issues)
    else:
        print(f"NOTE: {glp_dir} missing - German magazines (GLP add-on) not indexed")

    duplicates = len(issues) - len({i["key"] for i in issues})
    if duplicates:
        print(f"ERROR: {duplicates} duplicate keys")
        return 1
    # One publication row per (kind, name); a name shared across languages
    # would file the second language's issues under the first.
    langs: dict[tuple[str, str], set[str]] = {}
    for i in issues:
        langs.setdefault((i["kind"], i["publication"]), set()).add(i["language"])
    mixed = [k for k, v in langs.items() if len(v) > 1]
    if mixed:
        print(f"ERROR: publications with more than one language: {mixed}")
        return 1
    covers: dict[str, str] = {}
    for i in issues:
        other = covers.setdefault(i["cover_key"], i["title"])
        if other != i["title"]:
            print(f"ERROR: cover key collision: {other!r} vs {i['title']!r}")
            return 1

    out = root / "metadata" / "media.json.gz"
    payload = {"schema": 1, "issues": issues}
    with gzip.open(out, "wt", encoding="utf-8") as fh:
        json.dump(payload, fh, ensure_ascii=False)
    print(f"wrote {out} ({out.stat().st_size / 1024:.0f} KB, {len(issues)} issues)")

    articles = list(dict.fromkeys(collect_articles(helper) + glp_articles))
    art_out = root / "metadata" / "media_articles.txt"
    art_out.write_text("\n".join(articles) + "\n", encoding="utf-8")
    codes = len({a.split(";")[0] for a in articles})
    # eXo files a few mentions of one page under two kinds; those rows differ
    # only in `kind`, which is therefore part of the row's identity.
    kinds: dict[tuple[str, str, str], set[str]] = {}
    for a in articles:
        code, kind, page, entry = a.split(";", 3)
        kinds.setdefault((code, entry, page), set()).add(kind)
    shared = sum(len(v) for v in kinds.values() if len(v) > 1)
    print(f"wrote {art_out} ({len(articles)} articles, {codes} games, "
          f"{shared} rows sharing a page with another kind)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
