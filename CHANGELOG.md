# Changelog

## 0.13.2 - 2026-08-25

### Fixed

- On Windows, a large share of games opened the emulator and closed it again
  within a second or two, leaving an empty log behind - DOOM II and Relentless:
  Twinsen's Adventure among them. Their configuration mounts the game folder in
  a form that Windows rejected once Exodium had rewritten the path to where the
  games actually live, so no drive was mounted and nothing could start.
- Games whose configuration mounts a folder without quoting it could not start
  at all when the path to the game data contained a space. The mount stopped at
  the space and pointed at a folder that does not exist.
- A download brings its neighbours with it: game archives that share a piece
  with the one you asked for arrive complete and unasked. Those used to be
  added to My Library as if you had downloaded them - one library went from
  four games to seventeen. The library now follows what you asked for; the
  Rescan button and importing a folder still adopt whatever is on disk, because
  there the disk IS the answer.
- The prompt that offers to move older collection folders into one root now
  says what skipping it costs: Exodium only looks in the new root, so the games
  in the old folders read as not installed, and downloading one again writes a
  second copy while the first keeps its disk space.

### Added

- Per-game emulator choice on Windows: games tuned for DOSBox ECE can be run
  under DOSBox Staging instead, from the game's settings dialog. It is off by
  default - eXo picked ECE for a reason. The reason to switch is CRT shaders,
  which are a Staging feature; on ECE the setting is dropped rather than
  applied, and the dialog now says so next to the disabled control instead of
  leaving you looking for the file it got stuck in.

## 0.13.1 - 2026-08-14

### Fixed

- Spanish and Polish games that exist only in their language pack could be
  downloaded but not started - they had no DOSBox configuration, because the
  English catalog has no counterpart to borrow one from. Both packs' own
  configurations now ship with Exodium, which makes almost all of them
  playable. A handful remain for which eXo ships no configuration either.
- Many Spanish and Polish games appeared as their own card next to the English
  version of the same game. They are now language variants on one card, like
  the German ones, and use their own configuration when launched.

- Two entries for the collections themselves ("eXoDOS", "eXoWin9x") were listed
  as if they were games, and sorted to the very top of every list under All
  Collections. They come from eXo's own catalogs, which pin them there for the
  LaunchBox setup; existing libraries drop them on the next start.
- Language badges read as a mix of codes and full names, so the same language
  could appear as both "DE" and "GERMAN". The language packs write codes, the
  main eXoDOS catalog spells them out - all of them are now codes.
- Three German games (Sherlock Holmes: Serrated Scalpel, CyberMage, Die
  Kathedrale) offered no way to install them. Their catalog entries carry no
  file path, which is what the download normally keys on; they are now found by
  title and year instead.
- A preview video restarted from the beginning every second or so while another
  game's video was still downloading in the background. The same cause could
  keep an already-downloaded video from ever starting.
- The detail panel could show the placeholder instead of the cover when
  clicking quickly through games with a poster pack installed.

## 0.13.0 - 2026-08-14

### Added

- A tabular list view as an alternative to the cover grid, with sortable
  columns. The grid/list toggle sits at the top right of both tabs, and the
  My Library shelves follow the chosen view. (#21)
- Grid sections (letters, genres, years) now close with a divider, so the end
  of a section reads while scrolling. (#5)

### Changed

- The setup's first step now says plainly that Exodium creates subfolders
  (games plus covers and caches) under the chosen folder, instead of promising
  a bare game folder. Thanks to DerMicha75 for the feedback.
- A preview video that has just finished fetching starts playing right away -
  the cover already had its screen time during the download. The short pause
  before the video takes over remains for videos that are already on disk.

### Fixed

- German, Spanish and Polish versions of some games showed no cover in the
  detail panel, Gabriel Knight 2 among them. Their catalog rows carried a
  cover key belonging to a same-named game in a different collection (the
  Windows 9x pack catalogues many of the same titles), and every catalog
  update re-imported the broken keys from the bundled database. All
  cross-collection matching is now scoped to the pack family, the bundled
  catalog is regenerated, and a catalog refresh repairs the cover linkage
  itself instead of undoing it.
- The detail panel falls back to the English cover when a language variant's
  own cover file is missing, instead of showing an empty placeholder.
- The view toggle in My Library had its bottom edge sliced off by the first
  shelf title's sticky background.

## 0.12.2 - 2026-08-08

### Fixed

- The interface was drawn on the processor instead of the graphics card on
  Linux with NVIDIA, so scrolling, the detail panel and every animation ran at
  a fraction of the frame rate they should. A workaround for a startup crash
  from July switched off accelerated drawing for all of Linux, well beyond the
  crash it was meant to avoid. Exodium now picks the right path for the driver
  and display server it actually finds, and keeps the safe one everywhere it is
  still needed. Measured on an RTX 3080 under KDE Wayland: 10.6 to 60.8 frames
  per second. If an accelerated start ever fails, the next one falls back on
  its own.
- The AppImage could not reach that faster path at all, for two reasons of its
  own: it forced the X11 display backend on every system, and the WebKit it
  bundles refuses accelerated drawing on NVIDIA cards unless told otherwise.
  Both are fixed, which brings the AppImage to the same 60 frames per second.
  The same change also helps .deb users on distributions whose WebKit is older
  than 2.52, Ubuntu 24.04 among them.
- A test asserted a Unix-style path and could never pass on Windows, which had
  kept the Windows CI job red since the eXoWin9x merge.

## 0.12.1 - 2026-08-07

### Fixed

- Preview videos stayed black on Linux. 0.12.0 started serving them from a
  local HTTP server because WebKitGTK cannot play media from the asset
  protocol, but the app's content policy never allowed that server, so the
  player was blocked before it made a request. Both the fetched video and the
  detail panel's hero play again.
- Emulators hung on window close when Exodium ran as an AppImage, leaving the
  desktop to offer the kill dialog. The AppImage points its child processes at
  its own bundled libraries, so an emulator started from Exodium loaded a mix
  of those and the system's. They now start with a clean environment. The .deb
  and .rpm packages were never affected.
- The game detail panel stuttered as it slid in, most visibly on Linux. The
  media scan, the Windows 9x support check and the emulator probe all ran
  inside the animation; they now wait for it to finish. Game details, the
  language picker and the Manual button still appear immediately.

## 0.12.0 - 2026-08-07

### Added

- **New collection: eXoWin9x (Vol. 1, 1994-1996).** 662 Windows 95/98 games as
  the sixth collection. These boot a real Windows 9x from virtual hard-disk
  images: DOSBox-X runs most of the catalogue, 86Box covers the hardware-picky
  rest. On Windows both emulators come out of eXo's own support files; on
  macOS and Linux they install themselves as content packs alongside the first
  Win9x download (the Linux DOSBox-X is our own build - upstream publishes
  none). The shared OS images (~2.5 GB) also download automatically with the
  first Win9x game. Note: the median game is ~316 MB and boots a whole
  operating system - expect real downloads and slower first starts than DOS
  games. A handful of PCBox-tuned games stay unlaunchable for now.
- **Windows 9x online play.** Titles that dial the community-run IPX
  gateway can now do so on macOS and Linux: Exodium bridges the emulated
  network card once you grant packet access, offered on the first Play of such
  a game or in Settings → Network (and removable in the same place). This
  needs a **wired** connection on every system, Windows included: a Wi-Fi link
  cannot carry the emulated card's own hardware address (DOSBox-X documents the
  same limit for its pcap backend). Affected games now say so up front instead
  of failing with a Windows dial-up error. Single player is unaffected.
- **One folder for all collections.** Windows 3.x and 9x games used to land in
  their own folders next to eXoDOS; they now share the single tree eXo's own
  setup produces (`eXo/eXoDOS`, `eXo/eXoWin3x`, `eXo/eXoWin9x` side by side).
  Existing installs are asked once whether to move their files there - it is a
  move, not a copy, and nothing is downloaded again. Importing an existing eXo
  installation also stops missing the Windows packs, which previously looked
  uninstalled and invited a second download of the same games.
- **Moving the game folder now just works.** Exodium re-checks what is on
  disk at every start (and right after you point it at a different folder,
  reporting how many games it found), so install states follow the files
  instead of pointing at the old path until you find the button in Settings.
  It refuses to run that check against a folder holding no collection at all -
  an unmounted external drive can no longer make the whole library look gone.
  Repeated scans also stopped returning different numbers each time.
- **Rescan finds every collection.** "Rescan installed games" now walks each
  collection's own tree (it was hardcoded to eXoDOS's and missed eXoWin3x
  installs), and in-app manuals/screenshots from Win3x/Win9x game folders are
  no longer blocked by the asset-protocol scope.

### Fixed

- Opening a Windows 9x game on Windows no longer flashes two console windows
  (the emulator probe now runs windowless).
- The Windows 9x detail panel no longer claims "emulator not found" while the
  shared support files are still on their way: it shows the download's live
  progress, names the one-time ~2.5 GB payload next to the game's own size
  before the first download, and reports an extraction failure instead of
  showing progress forever.

## 0.11.0 - 2026-08-03

### Added

- **New collection: eXoWin3x.** 1,138 Windows 3.x games join eXoDOS and the
  language packs as a fifth collection - browsable, downloadable and launchable
  like the rest, running under the bundled DOSBox Staging. Existing installs
  pick it up automatically on the next start. Fair warning: the median Win3x
  game is ~237 MB (these ship as full machine images), so installs are real
  downloads rather than eXoDOS's click-and-play few megabytes.
- **Collection shelf.** The collection picker above the Browse grid is now a
  shelf of box covers - eXo's real section art for eXoDOS, the three language
  packs and eXoWin3x, each card lit in its own accent color when active, with
  its game count underneath. A new "All" card searches and browses the entire
  catalogue across collections in one view (and is the default).
- **Back to top.** Scrolling upwards in a long list shows a floating button
  back to the shelf; it stays out of the way while you scroll down.
- **eXoWin3x CD support under DOSBox Staging.** 55 Windows 3.x games ask for an
  IDE/ATAPI controller so the guest OS can see the CD (their CD driver runs
  inside the booted image, where DOSBox's usual CD emulation doesn't reach).
  Exodium now translates that request into Staging's `-ide` mount flag at
  launch - measured on a boot-image game: the drive attaches and CD playback
  works where the disc was previously invisible.
- **Printer heads-up.** 13 eXoDOS titles exist to print (The New Print Shop,
  The Newsroom, ...). DOSBox Staging cannot print yet, so their detail panel
  now says so instead of failing silently - the note disappears by itself on
  setups where eXo's DOSBox ECE build handles printing, and printer support is
  on its way into Staging upstream.
- The activity pill and the downloads indicator in the top bar merged into one:
  transfer rates, download progress and count in a single control whose flyout
  lists the downloads, the sharing status and a network-settings shortcut.

### Fixed

- The Manual button no longer looks clickable while a game's media is still
  being scanned (it shows a spinner), and a temporary scan failure no longer
  leaves it dead until the next app start.
- The screenshot lightbox no longer jumps back to the first image when the
  game's preview video finishes loading while you are browsing.
- The collection counts on the shelf no longer include a handful of catalogue
  rows that no collection filter can reach.
- "I Can Be a Dinosaur Finder" (eXoWin3x) is downloadable again: its launcher
  and its archive disagree in letter case, which the torrent matching now
  ignores (catalogue v5, applied to existing installs on next start).

## 0.10.0 - 2026-08-01

### Added

- **Game preview videos.** eXoDOS ships a short video for most games inside
  that game's extras archive - archives that run up to 1.1 GB. Exodium reads
  just the video out of them: the archive's index first, then only the video's
  own bytes, streamed from the torrent (one measured case: 27 MB fetched from a
  1163 MB archive). It starts when you open a game, plays in place of the box
  art, hands the cover back when it ends, and stays available in the
  screenshot lightbox. Fetches continue in the background when you close the
  panel, three at a time, so the video is simply there next time.
- **Offline mode**: setup now asks how Exodium should use the network. When
  importing an existing eXoDOS installation you can pick "Offline" - the
  torrent client is never started, and Exodium acts as a pure launcher for
  the games already on disk. Switchable any time in Settings → Network.
- **Network badge with live transfer rates.** The top bar shows the current
  down/up speed while anything is moving, summed across all four collections,
  and reads "Online" when idle. Its tooltip says what idle actually means -
  whether sharing is on, how many peers are connected, and how much has been
  shared this session - because a rate of zero on its own cannot tell "switched
  off" from "nobody is asking". Clicking it opens the network settings.
- **Speed limits.** Settings → Network takes an upload and download cap in
  KB/s, applied immediately and to the whole session. Empty means unlimited.

### Changed

- **The catalogue update notification was removed.** It could only tell you a
  new eXoDOS torrent exists and suggest a Factory Reset - which throws away
  favorites, playlists, install state and per-game settings. It comes back
  together with an update path that keeps your library (#18).

- **Settings use switches instead of checkboxes**, and every action button
  shares one component - a disabled button no longer lights up on hover as if
  it were clickable. Installing a content pack that needs the torrent is
  blocked while offline rather than explained afterwards, and the manual button
  stays inert until the file has actually arrived. Offline mode is shown in the
  top bar.
- **Sharing (seeding) is now an explicit choice.** Setup asks before it
  finishes - pre-selected, with the upload implications spelled out - instead
  of silently starting. Existing installs, which shared without ever being
  asked, get the same question once on startup; nothing is uploaded until it
  is answered. Changeable in Settings → Network.

- **Game detail panel reworked.** The panel is wider (scaled to the window)
  and puts the catalogue fields next to the description instead of below it,
  so art, actions, text and screenshots fit without scrolling. Multi-language
  games gained a language switcher: picking EN/DE/PL re-points the whole panel
  - play, download, uninstall, settings, manual, description and screenshots
  all refer to the selected version. Where a language has no text or manual of
  its own, the English one is shown and labelled as such.

### Fixed

- The lightbox thumbnail strip covered a preview video's seek bar, so the
  video could not be scrubbed.
- **Reinstalling a small game looked like a stalled download.** Torrent pieces
  are 8 MB and most games are far smaller, so a reinstall refetches the whole
  block the game shares with its neighbours - and the game's own progress stays
  at exactly 0% until that block arrives. The status said "no data received"
  and invited people to cancel a download that was working; it now distinguishes
  a shared-block wait from an actual stall.

- Multi-language games showed no description and their screenshot strip was
  cut off: the Versions list pushed the panel's flexible region to zero height.
- Multi-language games had no Manual or Settings button - both lived only in
  the single-language action bar, which the Versions list replaced.
- Screenshots in the detail panel appear immediately. The metadata pack ships
  full-resolution art (up to 18 MB per image) that the strip draws at 64x48;
  those are now downscaled once and cached on disk, so a gallery loads ~39 KB
  instead of ~2.7 MB. The lightbox still opens the originals.
- Covers no longer pop in while scrolling: they start loading about two screens
  ahead of the viewport instead of just before entering it.
- **Offline mode now means offline everywhere.** The first-run content-pack
  offer is skipped, pack installs are blocked (including the poster pack, which
  comes over HTTP), update checks are not made, and switching to offline stops
  a pack download that is already running.
- **DOSBox launch configs no longer pile up in your game folder.** Every
  launched game left a small `.conf` behind in the folder you chose for games,
  with nothing cleaning them up; they now live in Exodium's own directory and
  the strays are removed on the next start.
- Buttons with an icon aligned it on the text baseline instead of centring it.
- The first-run screen now shows Exodium's icon, and the window icon is
  Exodium's rather than the SolidJS logo left over from the project template.
- Preview thumbnails and the gallery cache were invisible to the app's own
  asset protocol - hidden directories are not matched by its scope, so nothing
  in them was ever served. Both caches moved out of hidden paths.
- Search now applies to My Library. The search box is visible on both tabs but
  only ever filtered Browse; typing while on My Library did nothing. Shelves
  (Recently Played, Favorites, Installed, playlists) now filter as you type,
  the jump bar follows, and a no-hits search offers to search the full
  collection instead.
- Download bars no longer flip between the percentage and the indeterminate
  sweep every few seconds. Torrent progress advances one piece at a time, so
  short flat stretches are normal; the bar now only sweeps after 15 seconds
  without data, matching the "waiting for peers…" status text.

## 0.9.0 - 2026-07-31

### Added

- **Playlists**: eXo's 11 curated playlists (MT-32, Sound Canvas, 3dfx,
  CGA Composite, GUS, PCjr, Quality Freeware, ...) ship with the catalog
  and are browsable via a new Playlists filter with a description banner;
  own playlists can be created from any game's right-click menu or the
  detail panel and appear as shelves in My Library (rename/delete via the
  shelf menu). Curated lists update automatically with catalog upgrades;
  user playlists survive them.

## 0.8.8 - 2026-07-30

### Fixed

- **Downloads could freeze at "Starting download..."** when several were
  queued in quick succession - two concurrent selection updates wedged the
  torrent engine so hard that even progress polls stopped returning.
- **AppImage failed to start on rolling-release distros** (Arch, Cosmic): the
  bundled Ubuntu copies of the GPU libraries broke EGL on newer Mesa, so they
  are no longer shipped inside the image.
- Download-sheet rows no longer re-animate on every poll tick, and variant
  downloads are labelled with their language.

## 0.8.7 - 2026-07-29

### Fixed

- Language variants of the same game no longer appear as duplicate cards on
  the Installed and Recently Played shelves.
- The cancel button on a card now targets the variant that is actually
  downloading.

## 0.8.6 - 2026-07-29

### Fixed

- **macOS: adding the eXoDOS torrent failed with EMFILE** on Macs whose
  per-process descriptor cap is scaled to installed RAM and sits below
  65,536. The limit is now raised to the kernel maximum instead of a fixed
  target (see 0.8.2 for why the engine needs that many).

## 0.8.5 - 2026-07-29

### Fixed

- **Linux: the AppImage aborted at startup on Wayland/NVIDIA**
  (`EGL_BAD_PARAMETER`) - WebKitGTK's DMA-BUF renderer is now disabled.

## 0.8.4 - 2026-07-29

### Fixed

- **macOS: fresh installs crashed on game launch** - the bundled DOSBox
  shaders now sit at the path DOSBox Staging actually looks in.
- **Multi-language games showed one card per language again**, with dead
  language badges; the merged rows from 0.8.3 are restored.
- Uninstalling a language variant no longer touches the other variants' save
  backups, and extraction/uninstall no longer race each other.
- Setup could end up with two torrent sessions that disagreed about state.
- SQLite writes now wait instead of failing when the database is busy.
- The CRT-shader toggle in Settings reflects the actual backend default, and
  the library no longer leaks a timer on every reload.

### Changed

- Windows ships the NSIS installer only; the MSI target is gone (the updater
  supports NSIS).

## 0.8.3 - 2026-07-27

### Added

- Support links: heart button in the top bar and a Settings section pointing
  to GitHub Sponsors and Ko-fi.
- Release pages now carry a "Which file do I need?" download table; README
  gained a quick-start section with direct-download permalinks.

### Changed

- Default log level is now `info` (librqbit debug chatter no longer floods
  the log); set `RUST_LOG` to raise verbosity for diagnosis.

## 0.8.2 - 2026-07-27

### Fixed

- **Linux: first download failed with "error opening ... in read/write
  mode"** - the torrent engine keeps every file of the 14,011-file torrent
  open, blowing Linux's default 1,024-descriptor limit at roughly file #950.
  The limit is now raised to the platform maximum at startup (standard
  torrent-client practice; macOS/Windows unaffected).
- Duplicated "Uninstalling" text in the detail panel.

## 0.8.1 - 2026-07-27

### Fixed

- **Launch, uninstall, and download of the same game are mutually
  exclusive** (per-game lock): the responsive UI made it possible to click
  Uninstall while a launch was still extracting the game - which could
  pollute or destroy the !save backup. The old sync design only prevented
  this by accident (the frozen UI made the click impossible).
- **Game-launch extraction runs on a blocking thread** instead of pinning a
  tokio worker; **right-click keeps its native menu inside text fields**
  (cut/copy/paste); the "Uninstalling" state no longer leaks onto another
  game's panel when switching mid-uninstall.
- **Content-pack images were blocked** since the v0.7.x asset-scope
  narrowing granted only the games subtree - screenshots and pack media in
  <data>/content now load again (field log showed 49 asset-protocol
  denials in one session).
- **Placeholder-cleanup log spam removed** (~14k debug lines per torrent
  re-add).

### Changed

- **The UI can no longer be frozen by backend work**: 27 commands (game
  launch incl. ZIP auto-extraction, game list queries, torrent parsing,
  metadata scans, config access) executed on the native main thread and
  froze all input while running - the full-list jump-bar fetch and busy
  moments around install/uninstall being the visible cases. All commands
  doing I/O now run on the async runtime; input stays responsive.
- **Native right-click menu suppressed** in production builds (the webview's
  Reload/Inspect menu doesn't belong in a launcher); the game cards' custom
  context menu is unaffected.
- **Uninstalling is a proper state**: all action buttons are replaced by an
  "Uninstalling…" label (detail panel, per-variant rows) with a subtle fade
  between states; sibling variant actions are disabled meanwhile.

## 0.8.0 - 2026-07-26

### Fixed (third adversarial review pass - 17 confirmed findings)

- **Uninstall no longer wipes the extras' download credit**: the piece
  ledger now only clears pieces of files actually deleted from disk - the
  still-present GameData ZIP keeps its credit, so reinstalling doesn't
  re-download gigabytes of extras.
- **Ledger restore survives Windows delete-pending** (exFAT/SMB/older NTFS):
  written via temp-file + rename with retries; failures now log at error
  level instead of silently reverting to the full re-check.
- **Uninstall during the extras phase** no longer leaves an orphaned poller
  that resurrects phantom stuck-download state for the removed game.
- **Cancel during validation sticks**: a deselect rejected by an
  initializing torrent is re-applied automatically once the check finishes
  (previously the cancelled game kept downloading invisibly).
- **Extras completion has a disk fallback** for librqbit's known stat-stall,
  and the extras phase resumes visibly after an app restart.
- **Disk preflight** credits on-disk bytes once (was double), and a refusal
  no longer leaves a phantom "My Games" entry.
- Selection updates hold the lock across the apply (closes a cancel race);
  install-moment refresh no longer skipped on re-downloads; UI reads an
  explicit installed flag instead of string-matching statuses.

### Fixed

- **The extras download phase is visible**: after a game installs, its
  GameData (manuals, videos, music - often larger than the game itself)
  keeps downloading; the card now shows "Installed - downloading extras…
  N%" instead of finishing silently, and when the extras land the manual
  button resolves automatically. Games stay playable the moment the game
  itself is installed.
- **Manual button tells the truth**: it now only appears for games that
  actually have a manual in the catalog, retries the lookup on click (the
  manual arrives inside the game's extras download, which often finishes
  after the game itself), and no longer caches a premature "no manual" for
  the whole session. Icon removed from the label.
- **Spanish/Polish metadata packs no longer show a 0 B download size** -
  the manifest carried placeholder sizes.

## 0.7.4 - 2026-07-26

### Fixed

- **No more "Validating torrent" after uninstall**: uninstall now patches
  librqbit's piece ledger surgically - only the removed game's pieces are
  cleared and the ledger is restored for the next add, which loads it via
  fastresume and starts downloading in seconds, exactly like a fresh
  install. The 15-30 minute full re-check (which field testing showed is
  slow regardless of antivirus or disk type) only remains for genuinely
  unrecoverable states (missing/corrupt ledger).

### Fixed

- **Uninstall is idempotent**: half-states (incomplete download, failed
  extraction) can be cleaned up instead of erroring with "not installed".
- **Detail panel no longer flickers** on install/uninstall completions -
  media state resets only when the displayed game actually changes.

## 0.7.3 - 2026-07-26

### Fixed

- **"Validating torrent" frozen forever after uninstall -> re-download**
  (observed twice on Windows): pushing a file-selection update into a
  torrent mid-initial-check could wedge librqbit's checking task. Selection
  updates now wait for the check to finish (without blocking progress
  polling) before applying.
- **librqbit upgraded** from a git-pinned 9.0.0-beta.2 to 9.0.0-rc.0 from
  crates.io - months of upstream fixes and no git-pin supply-chain
  dependency.

### Fixed (second adversarial review pass - 19 confirmed findings)

- **Linux deb/rpm installs no longer offered un-installable updates** - the
  tauri updater is AppImage-only on Linux; the update pill is now suppressed
  for package-manager installs.
- **Windows update flow asks before closing**: installing an update on
  Windows closes the app immediately (NSIS has no staged restart), so the
  pill now gets explicit confirmation first.
- **Support-file extraction is atomic**: staged to a temp dir and moved into
  place with renames, guarded by a process-wide lock, temp files cleaned on
  every path - a mid-extraction kill can no longer leave a silent
  half-extracted eXo/mt32 that every gate treats as complete forever.
- **Extraction watcher gains a disk-size fallback** - librqbit's per-file
  stat can stall short of total for fully-written files, and after a restart
  without session state the stats path never fires at all.
- **Cancelled downloads can't clobber retries**: a stale download_game
  promise from a cancelled attempt no longer overwrites a newer attempt's
  state with false errors.
- **Browse list fetches are epoch-guarded** - a slow background refresh can
  no longer drop an appended page or overwrite newer filter results.
- **Disk-space preflight credits bytes already on disk** - it was blocking
  exactly the resume/re-download recovery flows it should allow.
- **latest.json generation fails the release** if any platform's signed
  updater bundle is missing, instead of silently stranding that platform.
- Booter (`boot disk.img`) LP games no longer fall back to a generated
  autoexec; per-game CRT shader overrides skip DOSBox ECE; the asset
  protocol grant is narrowed to the eXoDOS media subtree; session eviction
  compares paths case-insensitively with proper boundaries on Windows/macOS;
  update check also runs after first-run setup; empty-state flash on cold
  start fixed; Escape guard made ordering-independent (capture phase).

## 0.7.2 - 2026-07-26

### Changed

- **Updates ask first**: a new release shows an "Update" pill in the top bar
  and a one-time toast - nothing downloads until you click it. After
  downloading, the pill turns into "Restart to update" and stays available
  until you're ready.

### Fixed

- **Factory reset clears recently-played history and per-game settings** -
  both previously survived a reset.
- **Manual button explains itself**: instead of silently disappearing when a
  game has no manual, it shows a disabled "No manual" state with a hint that
  manuals ship with the Metadata content pack.

## 0.7.1 - 2026-07-26

### Fixed

- **Support-file extraction survives restarts**: the watcher that extracts
  MT-32 ROMs / the ECE build from util.zip died with the app; if the 630 MB
  download finished in a later session, the assets never landed (observed in
  Windows testing). The watcher now re-arms at startup whenever util.zip is
  selected or on disk and the assets are still missing.
- **First-download feedback**: instead of sitting mute on "Starting
  download..." for minutes while the collection's 14,000 placeholder files
  are created (slow on Windows), the card now explains the one-time setup.
- **Log rotation**: exodium.log rotates at 10 MB keeping one predecessor -
  bounded size, and a wedged session can no longer destroy its own evidence.

## 0.7.0 - 2026-07-26

### Added

- **Auto-update**: Exodium checks GitHub releases at startup, downloads new
  versions in the background (signature-verified), and offers a one-click
  restart. Powered by tauri-plugin-updater; CI publishes a signed
  `latest.json` with every release.
- **DOSBox ECE on Windows**: games tuned for DOSBox ECE (~2,400) now run
  eXo's actual ECE build, extracted on demand from the collection's
  util.zip. On macOS/Linux they keep running under DOSBox Staging, with an
  "experience may vary" note in the game detail panel.
- **Toast notification system** (`stores/toasts.ts`, `ToastContainer.tsx`): download,
  uninstall, launch, and content-pack errors now surface as toasts instead of being
  silent or confined to inline status text. Includes a catalog-update notice on startup.
- **Hierarchical genre browsing**: genre sections and the jumpbar collapse
  " / "-delimited subgenres into ~15 top-level categories, matching the genre
  filter's new tree dropdown (`Select.tsx` depth rendering, `get_section_keys`
  parent collapsing).
- **README screenshots** and release plan under `docs/`.

### Changed

- **macOS: native titlebar** - macOS builds use the system traffic-light controls
  (`tauri.macos.conf.json` + runtime `set_decorations(true)`); the custom
  `WindowFrame` is now Linux/Windows-only.
- **Game detail panel rework**: pinned media strip, launch-button spinner, errors
  via toasts.
- Tab switches animate with a directional slide.

### Security / hardening

- **Seeding disclosure + toggle**: the setup flow now says plainly that
  Exodium joins the eXoDOS BitTorrent swarm and uploads while running, and
  Settings → Network gets a "Share with other players" toggle (default on;
  off caps upload at 1 KB/s, applied live and persisted).
- **Single-instance guard**: launching Exodium twice now focuses the existing
  window instead of corrupting the torrent session and contending on the DB.
- **Disk-space preflight**: downloads are refused upfront with a clear message
  when the data dir lacks space for the download plus extraction.
- **Narrowed asset-protocol scope**: was a blanket `**`/`$HOME/**`; now
  `$RESOURCE`/`$APPDATA` statically plus a runtime grant for the user-chosen
  data dir. A production Content-Security-Policy replaces `csp: null`.
- **DOSBox Staging's GPL license text** ships with the bundled binary
  (staged by get-dosbox.sh).

### Fixed

- **LP games launch via overlay mount** - the durable fix for the class of
  bugs where language-pack games flashed and exited (Cobra Mission ES et
  al.). Instead of guessing a launch command from directory contents, the
  EN config's autoexec now runs VERBATIM against a per-launch staging dir
  (`eXo/.exodium_lp/<lang>_<code>/`) whose `<code>` entry is a
  symlink/junction to the LP game dir. eXo's authored launch commands,
  CD imgmounts, and multi-step autoexecs all survive; an installed EN
  variant of the same game is shadowed correctly. A compatibility check
  (cd-chain simulation + launch-command verification) falls back to the
  old generated-autoexec heuristics only when the LP variant genuinely
  restructured the game.
- **Download stall feedback**: a download with no peers no longer sits at
  "0%" forever - after 15 s without progress the card shows "Looking for
  peers…", after 90 s an actionable stall warning. The premature
  "Download didn't start" verdict (which killed polling while the backend
  was still working) now waits for the backend command to actually resolve.
- **Browse keeps your scroll position**: background install/uninstall
  completion no longer resets the infinite-scroll list to page 1.
- **Jump bar stays in sync with search**, genre jump-bar keys can no longer
  point at sections that don't exist, and an empty search shows a
  "No results" state instead of a blank grid.
- **Content-pack install failures surface as toasts** even when the Settings
  dialog is closed (affects the first-run welcome flow).
- **Escape closes one overlay at a time** - manual/lightbox/settings no
  longer take the detail panel down with them.
- **Linux: PDF manuals** open via the system viewer with a clear hint -
  WebKitGTK has no inline PDF renderer, the old iframe stayed blank.
- **MT-32 / General MIDI music for ~2,900 games**: two stacked bugs left MIDI
  games silent or with wrong music. (1) The `!DOSmetadata.zip` download (15 MB:
  Roland MT-32 ROMs + SoundCanvas soundfont) never fired because the bundled
  configs zip pre-created the directory the check gated on - it now gates on
  `eXo/mt32/` itself. (2) ~1,500 configs use DOSBox-ECE key names
  (`[midi] mt32.romdir`, `fluid.soundfont`) that DOSBox Staging ignores -
  launch-time patching now translates them into Staging's `[mt32]` /
  `[fluidsynth]` sections (Staging-authored configs pass through unchanged).
  Correction during field testing: the ROMs actually live in
  `eXo/util/util.zip` (~630 MB), not `!DOSmetadata.zip` - the download is
  now fetched once, only when a game whose config requests MIDI is
  installed, and only the ~30 MB `mt32/` subtree is extracted.
- **38 games downloaded the wrong ZIP** (`find_game_files`): the torrent file
  matcher used an unanchored suffix match, so short titles matched longer ones -
  _Tetris_ fetched _Atomic Tetris_, _Pac-Man_ fetched _Ms. Pac-Man_, _Gods_
  fetched _Dusk of the Gods_, etc. The match is now anchored on a path boundary,
  the bundled DB is regenerated, and a regression test guards the collision set.
- **Versioned catalog refresh**: existing installs never re-read the bundled
  catalog, so fixes like the above (or a new eXoDOS torrent) would only reach
  fresh installs. A `catalog_version` check at startup now updates catalog rows
  in place - user state (installed, library, favorites, per-game config)
  and `games.id` are preserved.
- **Cross-collection placeholder cleanup**: downloading a game 10 s after a
  game from another collection could delete the first torrent's tracked 0-byte
  placeholders (all four collections overlay one root), reintroducing the
  "100% but ZIP missing" loop. Cleanup now keeps the union of all enabled
  collections' file lists.
- **Interrupted downloads resume after restart**: the download manager now
  adopts torrents auto-resumed from librqbit's session persistence (handle +
  file selection), and merges instead of replaces the selection when the
  session already manages a torrent. Previously a download in flight at
  shutdown kept downloading invisibly and the next download silently
  deselected it.
- **Uninstall → re-download stuck at 100%**: uninstalling deletes the game
  ZIP, but librqbit's fastresume bitfield still claimed its pieces existed,
  so a re-download instantly reported 100% with no file on disk. Uninstall
  now drops the torrent from the session (removing its fastresume state) and
  re-adds any still-selected files; the next download re-derives piece state
  from disk.
- **Startup failures show an error dialog** instead of a silent crash
  (unresolvable data dir, unreadable/uninstallable database).
- **LP games launch from unextracted ZIPs**: launch-time auto-extraction only
  looked for the EN ZIP location; it now also checks the language-pack dir.
- **Cancelling a download keeps the shared EN GameData** when another
  language variant of the game is still downloading.
- **macOS: DOSBox launch EBADF** - Tauri 2 GUI builds hit `posix_spawn` EBADF when
  redirecting DOSBox stdio to log files. On macOS stdio is now nulled and a no-op
  `pre_exec` forces fork+exec; other platforms keep per-game DOSBox log files.

- **LP game launch - commented-out autoexec** (`patch_dosbox_conf`): LP games whose
  `dosbox.conf` has the game-launch lines commented out with `#` (e.g. _Das Amt_) now
  launch correctly. When Strategy 1 (redirect EN config) produces an autoexec with no
  executable command, `find_lp_launch` is called to locate the real launcher by
  inspecting the game directory.

- **LP game launch - extended launcher discovery** (`find_lp_launch`): Added two new
  fallback strategies beyond the existing `run.bat` / `.com` search:
  - **Strategy 2** - scans for any `.bat` file (excluding known utilities like
    `anleit`, `install`, `problem`) that calls a `.exe` or `.com`; returns the `.bat`
    itself so all its steps execute in sequence.
  - **Strategy 4** - looks for a `.exe` in named subdirectories, skipping DOS/4GW
    extenders (`rtm`, `dos4gw`, `dpmi`, `cwsdpmi`) and installers.

- **"Download incomplete" false positive** (`get_download_progress`): Games like
  _Captain Zins_ and _Skyworker_ could show a permanent "Download incomplete" error
  even though their download had never been attempted. Root cause: torrent pieces
  received while downloading a neighbouring file can cover a small game's bytes
  entirely, causing librqbit to report 100% for that file before it is ever selected
  - the file is therefore never assembled on disk. The code now re-requests file
  assembly via `download_files` (which calls `update_only_files`) and keeps polling
  rather than surfacing an error.

### Changed

- `autoexec_has_launch_cmd`: drive-switch detection generalised from a hard-coded
  `c:`/`d:`/`e:`/`f:` list to any single ASCII letter followed by `:`, covering
  floppy drives (`a:`, `b:`) and drives above `f:`. Also added `echo ` and `@exit`
  to the non-launch filter list.

- `DownloadManager`: new `is_file_selected` method used to gate re-trigger spawns in
  `get_download_progress`, preventing a new task being spawned on every 1-second poll
  while librqbit assembles the file.

### Added

- **Test suite**:
  - Frontend: `vitest` + `jsdom` wired up; `pnpm test` / `pnpm run test:watch` /
    `pnpm run test:all`.
  - Rust: `tempfile` + `pretty_assertions` dev-dependencies; tests for
    `queries` (insert/fetch, language merging, config), `import/xml`
    (shortcode extraction, LP path handling, full XML parse round-trip), and
    `commands/games` (`patch_dosbox_conf`, `find_lp_launch`,
    `collection_data_dir`).
