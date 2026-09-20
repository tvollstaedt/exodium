# Exodium v1.0 Release Plan

*Generated 2026-07-12 from a 7-agent deep audit (backend, frontend/UX, release engineering, eXoDOS feature parity, tests, uncommitted-diff review, completeness critic). All findings below cite verified code evidence unless marked "likely".*

> **Status 2026-07-12:** Phases 0, 1 are DONE (incl. an adversarial review
> pass; fixes landed for its findings - see CHANGELOG). Phase 2 core is DONE
> (MT-32/soundfont download gate + ECE→Staging key translation, unit-tested);
> Phase 5 core is DONE (ci.yml test job on push/PR, clippy clean, import
> smoke tests, RPM upload).
>
> Phase 3 pre-release UX items are DONE (stall feedback, Browse scroll
> retention, jumpbar sync, empty state, content-pack toasts, Escape guard,
> Linux PDF fallback). Phase 4 code hardening is DONE (single-instance,
> disk-space preflight, asset scope + CSP, GPL license staging).
>
> **RELEASED**: v0.7.1-v0.7.3 are public; v0.7.3 is the full "latest"
> release with auto-update live (consent pill), DOSBox ECE on Windows,
> librqbit 9.0.0-rc.0, and fixes from two adversarial review passes plus
> three rounds of Windows field testing (install, LP junction overlay,
> uninstall/re-download, auto-update chain all verified in the field).
> Remaining before announcing: v0.7.3 Windows re-test of the
> uninstall->re-download flow with the Defender exclusion set, ECE engine
> log-line verification, one Linux VM smoke test, macOS notarization
> decision, and cleanup of stale pre-0.7 releases/drafts.
>
> Older status (superseded):
> Seeding disclosure + Settings toggle (default on) and in-app eXoDOS
> attribution are DONE. **Everything left is manual/user territory:**
> (a) smoke-test the CSP + asset-scope change - launch the app and confirm
> thumbnails, screenshots, and manuals render; (b) live-torrent test of
> download → uninstall → re-download; (c) MIDI parity listening matrix
> (MT-32/GM/tandy/pcjr/CGA/GUS/voodoo); (d) push + first 3-OS CI run;
> (e) eXo approval covering the full redistribution surface;
> (f) signing/notarization + macOS-Intel decisions; (g) release surface
> (/releases/latest currently points at the content pack); (h) version bump
> + tag when ready. Post-release: keyboard nav, restart-resume UX, glshader
> override + backslash rewrite narrowing.

## Verdict

**No design rehaul needed.** The frontend reads as a releasable Steam-like launcher with a coherent dark design system. CI already builds installers for all three OSes (verified green through v0.6.7). Both test suites pass (26/26 vitest, 36/36 cargo).

What actually stands between 0.6.9 and a public v1.0:

1. **One confirmed data blocker** — 34 games in the shipped catalog point at the *wrong* torrent file.
2. **A feature blocker you already suspected** — MIDI music is broken/silent for ~2,900 games under DOSBox Staging.
3. **Torrent-lifecycle bugs** that produce the known "stuck download" UX.
4. **Legal/positioning sign-offs** — eXo approval scope, GPL compliance, seeding disclosure.
5. **A testing story** for the OSes you can't run — CI currently runs zero tests.

Estimated effort: **~3–4 focused weeks** (Phases 1, 2, 5 partly parallelizable).

---

## Phase 0 — Repo hygiene (half a day)

The public repo is currently broken for visitors and the working tree tangles ~8 themes.

- [ ] **Untangle and commit the 936-line WIP** as separate commits: (1) em-dash→hyphen sweep (~⅔ of diff lines), (2) macOS batch (native decorations, DOSBox spawn EBADF fix), (3) toast system, (4) genre hierarchy UX, (5) detail-panel rework. The macOS spawn fix deserves its own commit for easy revert.
- [ ] **Commit `docs/screenshots/` with the README changes** — README image links 404 on the public repo right now.
- [ ] **Delete `src-tauri/exodium.db`** (stray 0-byte artifact) and add `src-tauri/*.db` to `.gitignore`.
- [ ] **Decide `tauri.macos.conf.json`**: commit or delete — it's redundant with the runtime `set_decorations` call; leaving it untracked makes CI-built macOS builds diverge from local testing.
- [ ] **Fix `Cargo.toml` version**: 0.3.2 → match tauri.conf.json.
- [ ] **Fix or defer the catalog-update toast** — as wired it can never fire: `init_download_manager` overwrites the stored infohash before the check runs. Correct fix: check before init, or store the hash at import time.
- [ ] **Genre jumpbar/section mismatch** (in the WIP diff): jumpbar can list genres that have no rendered section; clicking loads all 7,600 games then silently fails to scroll.
- [ ] **Update CHANGELOG** for the in-flight features.

## Phase 1 — Correctness blockers (~1 week)

### 1.1 Wrong-torrent-file mapping (BLOCKER, confirmed, fix = hours + migration)

`find_game_files` (`src-tauri/src/torrent/mod.rs:94`) matches with an unanchored `ends_with(&game_zip)`. **34 games get the wrong first match**: Billiards→4 Balls Billiards, Carnage→Alien Carnage, Gods→Dusk of the Gods, Columns→Beyond Columns, The Incredible Machine→Even More Incredible Machine… User downloads the wrong ZIP, it "installs", then launch fails forever with "Game files not found — re-download".

- Fix the match to anchor on a path boundary: `ends_with(&format!("/{}", game_zip))`.
- The bug lives in **two code paths**: `generate_db.rs:155` (bundled DB) *and* the runtime backfill in `setup.rs:1323`.
- Regenerate `metadata/exodium.db`.
- Add a regression test for a collision title (e.g. "Billiards (1993)").
- **Requires 1.2 to reach existing users.**

### 1.2 Catalog upgrade path (required by 1.1)

`lib.rs:220` installs the bundled DB only when missing/empty; `db::migrate` only adds columns. Existing users never receive a corrected catalog. Add a `catalog_version` config key checked at startup; when it changes, re-import catalog rows (torrent indices, sizes, variants) while preserving user state (`installed`, `in_library`, `favorited`, `last_played`, config) — join on shortcode+language.

### 1.3 Cross-collection placeholder cleanup (high, confirmed, hours)

`cleanup_placeholder_files` (`manager.rs:67–111`) walks the shared overlay root but keeps only the *adding* torrent's file list — downloading a GLP game 10s later deletes the eXoDOS torrent's tracked 0-byte placeholders. Same failure class as the v0.6.6 bug fixed in v0.6.7, now cross-collection. Fix: pass the union of all initialized managers' file lists, or restrict cleanup to the collection's own subdirectory.

### 1.4 Fastresume hydration (high, confirmed, days)

Known issue from 0.6.4+, still unfixed: `DownloadManager` starts with `handle=None`, so after restart, in-flight downloads show no progress and never extract; the next `add_torrent` returns `AlreadyManaged` and *replaces* the file selection, silently deselecting everything that was still downloading. Fix: at manager init, look up the torrent in the persisted session by info-hash, hydrate `handle` + `selected_files` (from librqbit `only_files` or DB `in_library && !installed` rows); make the `AlreadyManaged` path merge selections.

### 1.5 Download → uninstall → re-download loop (high, likely — TEST THIS)

`uninstall_game` deletes the ZIP but librqbit's fastresume bitfield still marks the pieces present. Re-download reports 100% instantly, ZIP missing, recovery loop spins ~5 min then gives advice that repeats the failure. This is a core user flow. Test end-to-end on one platform; if confirmed, invalidate affected piece ranges in the `.bitv` (or force a recheck) on uninstall.

### 1.6 Startup panics → error dialog (medium, hours)

`lib.rs:212–252` has an `.expect()` chain (data dir, DB open, migrations). On a broken install the app dies with no visible message — replace with an error dialog + graceful exit.

### Also in this phase (small)

- LP games in ZIP-not-yet-extracted state: `launch_game` auto-extract only checks the EN ZIP location.
- `cancel_download` deselects the shared EN GameData file other in-flight LP downloads may need.
- `Instant` arithmetic panic risk in `get_download_progress` shortly after boot.

## Phase 2 — MIDI audio, the real feature gap (~1 week)

Your instinct about "properly supporting different dosbox settings" was right, but the audit narrowed it: per-game `dosbox.conf` files **are** honored (machine type, cycles, mounts, autoexec all apply). What breaks is the **audio chain**:

- **694 games** configured for Roland MT-32 and **~2,200** referencing the SC-55 soundfont use ECE-specific config keys (`[midi] mt32.romdir`, `fluid.soundfont`) that DOSBox Staging doesn't recognize.
- The ROMs/soundfont live in the torrent's `!DOSmetadata.zip` (`eXo/mt32/`), but **that download is dead code**: the bundled configs zip pre-creates the `!dos` directory that gates it (`games.rs:223`), so the support tree never lands on disk. Confirmed by the critic's spot-check.

Result: roughly a third of the catalog plays with silent or wrong music — against the core "everything preconfigured" value proposition.

- [ ] Fix the `!DOSmetadata.zip` gate (or fetch just `eXo/mt32/`: 2 ROMs + SoundCanvas.sf2 from the user's own torrent — keeps Roland ROMs out of the app binary, better legal posture).
- [ ] Translate ECE→Staging config keys in `patch_dosbox_conf` at launch (`mt32.romdir` → Staging's MT-32 settings, `fluid.soundfont` → Staging FluidSynth).
- [ ] Also fix: global override force-overriding per-game `glshader`/`fullscreen`; blanket backslash→slash replacement can mangle DOS-internal paths in autoexec.
- [ ] Spot-test parity matrix: 1× MT-32, 1× GM/soundfont, 1× `machine=tandy`, 1× `pcjr`, 1× CGA composite, 1× GUS, 1× `voodoo=true` (e.g. Tomb Raider).
- [ ] README/first-run compat note: ~19 games tuned for ECE/DOSBox-X specials (3dfx tuning, GunStick) may misbehave.

**Defer to v1.x with a public roadmap:** playlists (bundled data + empty DB tables already exist), per-game Extras/setup-utility launcher, gameplay videos, ScummVM runtime, custom mapper files (SDL1→SDL2 incompatible). Document as out of scope: the Media Pack's soundtracks and videos (its magazines, books and catalogs ship as the Lesesaal, §19).

## Phase 3 — UX polish, no redesign (~1 week)

Frontend auditor's verdict: design system is coherent and releasable; spend everything on error/edge states.

**Pre-release:**
- [ ] Stalled-download feedback: no progress delta for N seconds → "Looking for peers…" → actionable error with Retry. Currently 0% forever with no feedback.
- [ ] Stop background install/uninstall completion resetting the Browse infinite-scroll to page 1.
- [ ] Toast content-pack install failures (currently silent unless Settings is open — affects first-run).
- [ ] Refresh jump-bar section keys on search; add "No results for …" empty state.
- [ ] Escape closes stacked dialog AND detail panel in one press — guard it.
- [ ] **Verify PDF manuals on Linux** — WebKitGTK likely renders a blank iframe; fall back to `openPath()`. Highest-risk untested-OS item in the frontend.
- [ ] False "Download didn't start" verdict after 5 s kills polling while backend may still succeed.
- [ ] Change-data-dir flow: no error handling, installed games left pointing at the old location.

**Post-release acceptable:** keyboard navigation for game cards (library is mouse-only), focus trap in detail panel, restart-mid-download resume UX, setup-phase percent progress, 5s library polling forever.

## Phase 4 — Hardening + legal (before publicizing, ~2–3 days code + external waits)

- [ ] **Seeding disclosure**: the app joins a public swarm and uploads copyrighted content on every launch — no disclosure, toggle, or upload limit. Add a first-run notice, a seeding indicator, and a toggle/limit (or pause-on-complete).
- [ ] **eXo approval must cover the full redistribution surface**: the four `.torrent` files, the committed box-art thumbnails, the hosted posters pack, and the metadata XMLs — not just the torrents. A DMCA takedown of the repo is the project's biggest external risk. (Per prior eXo communication this is the #1 blocker; emulator-compat quality — Phase 2 — is exactly what they care about.)
- [ ] **GPL compliance**: bundle DOSBox Staging's COPYING + attribution + source offer (stage via `get-dosbox.sh`).
- [ ] `tauri-plugin-single-instance` — double launch currently corrupts the torrent session and contends on SQLite.
- [ ] Disk-space preflight before multi-GB download + extraction.
- [ ] Narrow Tauri `assetProtocol` scope from `**` / `$HOME/**` to the served directories; set a CSP (currently `null`).
- [ ] In-app attribution for the eXoDOS project.

## Phase 5 — CI, tests, and the cross-OS problem (parallel to 1–3)

Addresses "I struggle to test on different OSes". CI currently builds but **runs zero tests** and only triggers on tags.

- [ ] **New `ci.yml` on push/PR, 3-OS matrix**: `cargo test`, `pnpm vitest run`, `tsc --noEmit` (add a `typecheck` script), `cargo clippy -- -D warnings`.
- [ ] **CI-runnable smoke tests** (pure Rust, no torrents): full bundled-metadata import (assert ~7,600 games + PLP/SLP shortcode backfill), torrent-matcher regression (the 1.1 fix), DB-migration upgrade test (open a 0.6-shaped DB, migrate, assert), uninstall/save-backup/restore cycle on temp dirs.
- [ ] **Release surface fixes**: GitHub `/releases/latest` currently resolves to the content pack, not an installer — mark `content-v3` as not-latest and publish app releases as latest. Upload the already-built RPM (one-line glob change).
- [ ] **macOS Intel decision**: add a `macos-13` matrix entry or state "Apple Silicon only" in README.
- [ ] **Signing/notarization decision**: unsigned builds mean scary first-run dialogs on macOS/Windows; notarization is the biggest drop-off risk. Budget it or document workarounds prominently. Remove the inert `TAURI_SIGNING_*` env or wire `tauri-plugin-updater`.
- [ ] **Manual per-OS smoke checklist** (recruit 1–2 testers per OS): install → first-run wizard → download one small game → launch (verifies bundled DLLs/sidecar) → MIDI audio → PDF manual → custom titlebar drag/resize → uninstall/reinstall.
- [ ] **Linux VM end-to-end before tagging** — the critic flagged that the primary target OS has plausibly never executed the app end-to-end.
- [ ] Fix `playability.rs` env-var name mismatch (`EXODIAN_DATA_DIR` vs `EXODIUM_DATA_DIR`).

## Suggested sequence

```
Week 1   Phase 0 (½ day) → Phase 1 blockers  ┐
         Phase 5 CI + smoke tests            ┘ parallel
Week 2   Phase 2 MIDI chain + parity matrix
Week 3   Phase 3 UX polish + Phase 4 hardening
         → contact eXo with the full redistribution surface + Phase 2 results
Week 4   Tag v0.7.0 as public release candidate, recruit per-OS testers,
         fix fallout → v1.0
```

Low-risk supply-chain note: `librqbit` is pinned to a git-tag beta (`v9.0.0-beta.2`) — verify `Cargo.lock` is committed and consider pinning to a crates.io release before v1.0.
