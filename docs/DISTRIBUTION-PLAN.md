# Distribution Plan

*Drafted 2026-09-21. Goal: make Exodium installable through the package
managers people already use, without taking on the official distro
repositories, which are out of reach for this project (see "Not pursued").*

Today the release workflow publishes a DMG, an AppImage, a .deb, an .rpm and
an NSIS installer to GitHub Releases, and the app updates itself from
`releases/latest/download/latest.json`. Every channel below repackages those
artefacts or builds from the same sources; none of them replaces the GitHub
release, which stays the canonical one.

## Tier 1 - repackage the release artefacts (cheap, self-hosted)

No third-party review gate, all fed automatically by the release workflow.

- [ ] **Homebrew tap** (`tvollstaedt/homebrew-exodium`): a cask wrapping the
      DMG. The official `homebrew/cask` tap has a popularity threshold the
      project does not meet and discourages unsigned apps; an own tap has
      neither gate. Until the app is notarized, users still see Gatekeeper's
      "damaged app" dialog and need `--no-quarantine` - the cask should say so.
- [ ] **AUR `exodium-bin`**: PKGBUILD extracting the .deb (or the AppImage),
      depending on `webkit2gtk-4.1`, `gstreamer` plugins and `dosbox-staging`
      from the repos. A source PKGBUILD (`exodium`) is possible later; the AUR
      allows network access during build, so `pnpm install` + `cargo` work.
- [ ] **Scoop bucket** (`tvollstaedt/scoop-exodium`): JSON manifest on the
      NSIS installer, `autoupdate` on the release tag.
- [ ] **winget**: manifest PR to `microsoft/winget-pkgs`. Unsigned installers
      are accepted; SmartScreen still warns until the exe is Authenticode-signed.
- [ ] **apt + dnf repositories** on GitHub Pages: `reprepro`/`aptly` for the
      .deb, `createrepo_c` for the .rpm, one GPG key for both. Users add one
      source line and get updates through their package manager. Covers
      Debian, Ubuntu, Fedora, openSUSE without any of their review processes.
- [ ] **Release workflow**: after the GitHub release, bump the tap, bucket and
      PKGBUILD, and regenerate the apt/dnf indexes. Version and hashes come
      from the release assets, nothing is hand-edited.

### Prerequisites shared by every packaged install

These apply as soon as anything other than the GitHub installer puts Exodium
on a machine, and to any future source build (AUR source package, Flatpak).

- [ ] **Build without the DOSBox Staging sidecar.** `tauri build` refuses to
      run when an `externalBin` is missing, and distro packages must not ship
      a vendored emulator. A build feature (or a config overlay dropping the
      sidecar) is needed; the runtime already falls back to `dosbox-staging`
      on PATH.
- [ ] **Shader lookup for a system DOSBox.** The launcher stages `shaders/`
      and `shader-presets/` from the bundle into DOSBox's config dir. A distro
      package of DOSBox Staging carries them under `/usr/share/dosbox-staging`;
      the staging step needs that fallback or CRT modes silently degrade.
- [ ] **Disable the in-app updater for package-managed installs.** Otherwise
      the updater downloads a release and tries to replace files the package
      manager owns. Detect at build time (feature flag set by the package) or
      at runtime (install prefix under `/usr`, `/opt/homebrew`, AppImage vs
      not), and hide the update prompt with a "managed by your package
      manager" note in Settings → About.
- [ ] **Code signing.** macOS notarization and Windows Authenticode are what
      make the Homebrew and winget channels actually pleasant; without them
      every channel inherits the first-run warning. Tracked with the v1.0
      release plan, not here.

## Tier 2 - Flathub (later)

Tauri apps ship on Flathub and the Tauri docs cover the manifest, so the path
exists, but the cost is in the sandbox, not the build:

- `cargo-sources.json` and offline Node sources must be generated; the
  Flatpak node generator handles npm/yarn lockfiles well and pnpm poorly.
- DOSBox Staging has to be built as a module inside the manifest (the
  `io.github.dosbox-staging` Flatpak cannot be called from another sandbox).
- The emulator content packs are AppImages and cannot run inside a Flatpak.
  DOSBox-X and 86Box would need the same treatment as Staging, or
  `flatpak-spawn --host` with a matching permission.
- Raw-socket multiplayer (`pkexec setcap` on the emulator) and the per-GPU
  render-path selection both assume an unsandboxed host.
- Flathub reviews what an app does, and a launcher that ships the collection
  torrents is a grey area there. Fetching the torrents at first run (below)
  reduces what the package itself carries, but does not change the purpose.

Revisit after signing is settled and the patched librqbit fork is upstream.

## Fetch the torrents at first run instead of bundling them

Related: the catalogue-update issue (#18) - the torrent version *is* the
catalogue version.

Why: the `.torrent` files are the one artefact every packaging review looks
at first, and bundling ties a torrent update to an app release. Why not more
than that: the files are not what makes the app legally interesting, its
purpose is, so this is package hygiene, not a change in exposure.

Constraints that shape the design:

- **The bundled database bakes in torrent file indices.** A torrent with a
  different file order invalidates every stored index. The fetched file must
  therefore be pinned to the infohash in `manifest.json` and verified against
  it before use - never "the latest torrent from the source".
- **Offline mode still needs the index.** Import matching, installed-game
  sizing and the overlay-variant detection all parse the torrent files even
  when no session is ever created. Sizing degrades gracefully without them,
  import matching does not. The fetch has to happen once, before the offline
  choice in setup.
- **No magnet-only start.** Resolving metadata over DHT takes a minute on a
  cold start, needs a session, and offline has none. The first-run import
  would block on it.

Design:

- [ ] Host the `.torrent` files on a content release (same pipeline as the
      poster and emulator packs: URL + SHA-256 + size in `manifest.json`).
      Hotlinking them from the collection's own website would make setup
      depend on a third party's server and goodwill.
- [ ] `bundled_torrent_path` becomes `torrent_path`: look under
      `<data_dir>/content/torrents/` first, then the bundle if present
      (transition), else fetch. Every consumer already goes through that one
      function.
- [ ] Setup gains a "Fetching catalogue indexes" step before import and before
      the network-mode choice; failure is retryable and blocks only the
      import route that needs the index.
- [ ] Verify infohash on every load, not only after download - a stale cached
      file after a manifest bump must be refetched, not trusted.
- [ ] Drop `../torrents` from the bundle resources once the transition is
      over; keep the files in the repo for `generate_db` and the tests until
      those read them from the same cache.

## Not pursued: official distribution repositories

Debian main, Ubuntu, Fedora and Arch `extra` are out of reach and would stay
so even after the work above:

- Every Rust crate must be packaged separately, and Tauri v2 is in none of
  them. librqbit runs from a patched Git fork (deadlock fix, upstream PR
  pending) and is unpackageable until that is merged and released.
- No vendored binaries, no network during the build, and `node_modules` from
  distro packages only.
- The project's purpose - downloading the collection's copyrighted game
  archives - is something the Debian FTP masters and Fedora Legal reject on
  its own, independent of the packaging.

The apt/dnf repositories in Tier 1 give those users a package-manager install
without any of this.
