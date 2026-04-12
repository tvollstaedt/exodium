<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" width="128" alt="Exodium" />
</p>

<h1 align="center">Exodium</h1>

<p align="center">
  A cross-platform launcher for the <a href="https://www.retro-exo.com/exodos.html">eXoDOS</a> collection. Browse, download, and play DOS games on Linux, macOS, and Windows.
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License: MIT" /></a>
  <img src="https://img.shields.io/badge/platform-Linux%20%7C%20macOS%20%7C%20Windows-blue" alt="Platform" />
  <img src="https://img.shields.io/badge/built%20with-Tauri-blueviolet" alt="Built with Tauri" />
  <img src="https://img.shields.io/badge/language-Rust%20%2B%20TypeScript-orange" alt="Rust + TypeScript" />
</p>

---

## A tribute to eXoDOS

Exodium would not exist without the extraordinary work of the [eXoDOS project](https://www.retro-exo.com/exodos.html) and its creator, **eXo**. Over many years, eXo and the eXoDOS community have painstakingly collected, configured, and preserved over 9,000 DOS games, each one pre-configured to run out of the box. The result is an irreplaceable archive of gaming history.

Exodium is a frontend client for that collection. It does not host or distribute any game files; it uses the eXoDOS torrents that you seed yourself. If you find value in Exodium, please consider supporting the eXoDOS project directly at [retro-exo.com](https://www.retro-exo.com).

---

## What it does

eXoDOS ships with a Windows-only LaunchBox frontend and requires downloading the full ~500 GB torrent to browse the catalogue. Exodium replaces that frontend with a native app that:

- **Runs everywhere** - Linux, macOS, Windows
- **Streams on demand** - download only the games you want, no full collection required
- **Zero dependencies** - DOSBox Staging is bundled, no separate installation needed
- **Favorites** - star games to bookmark them for later

---

## Installation

Download the binary for your platform from the [latest release](https://github.com/tvollstaedt/exodium/releases/latest).

### macOS

Because Exodium is not yet signed with an Apple Developer ID, macOS Gatekeeper will block it on first launch with "Exodium is damaged and cannot be opened". To bypass this, run the following once after dragging the app to Applications:

```bash
xattr -cr /Applications/Exodium.app
```

This removes the quarantine attribute that macOS adds to downloaded files. The app is otherwise unmodified — the binary itself is built and distributed directly from this repo's CI pipeline.

### Linux

Install the `.deb` (Debian/Ubuntu) or run the `.AppImage` directly (any distro). The AppImage needs `chmod +x` first.

### Windows

Run the `.msi` installer. SmartScreen may warn about an unsigned publisher — click "More info" → "Run anyway".

---

## Features

### Available now
- ✅ Browse ~9,200 games across eXoDOS + German, Spanish, and Polish language packs
- ✅ Stream individual games on demand - no full collection download required
- ✅ Launch via bundled DOSBox Staging with no external dependencies
- ✅ Favorites and a personal library of installed games

### Planned
- 🔲 Full metadata support including manuals, game videos, and other media from the eXoDOS archive
- 🔲 Individual game settings with a per-game DOSBox configuration editor
- 🔲 Improved download management - queue, pause/resume, bandwidth throttling, progress history
- 🔲 Support for other eXo collections - eXoWin3x, eXoWin9x, eXoScummVM, eXoDream, and future releases

---

## Tech stack

| Layer | Technology |
|-------|-----------|
| Shell | [Tauri v2](https://tauri.app) (frameless window, `decorations: false`) |
| Frontend | [SolidJS](https://solidjs.com) + TypeScript + Vite |
| UI Components | [Ark UI](https://ark-ui.com) headless (`@ark-ui/solid`) |
| Backend | Rust |
| Database | SQLite via `rusqlite` (WAL mode, pre-built and shipped with the app) |
| Torrent | [librqbit](https://github.com/ikatson/rqbit) with selective file downloads |

---

## Development

### Prerequisites

- [pnpm](https://pnpm.io)
- [Rust toolchain](https://rustup.rs) - bootstrapped automatically by `pnpm tauri dev` if not present
- [aria2](https://aria2.github.io) - for `init-dev` only (`brew install aria2` / `apt install aria2`)
- Python 3 + [Pillow](https://python-pillow.org) - for `init-dev` only (`pip3 install Pillow`)

> DOSBox Staging is downloaded automatically by `init-dev`. No manual installation needed.

### First-time setup

```bash
pnpm install

# Download thumbnails and the DOSBox binary (one-time, ~2-5 GB depending on language packs)
pnpm run init-dev

pnpm tauri dev
```

`init-dev` is idempotent - already-downloaded files and existing thumbnails are skipped. Use `--force` to regenerate thumbnails. Data is cached at `~/.exodium-dev/` (override with `XDO_DEV_DATA=/your/path`).

Language pack options:

```bash
pnpm run init-dev --glp     # German Language Pack (~23 GB)
pnpm run init-dev --slp     # Spanish Language Pack (~3.8 GB)
pnpm run init-dev --plp     # Polish Language Pack (~800 MB)
pnpm run init-dev --all-packs
```

### Useful scripts

| Command | Description |
|---------|-------------|
| `pnpm tauri dev` | Start the app in development mode |
| `pnpm run init-dev` | First-time setup: DOSBox binary + thumbnails |
| `pnpm run get-dosbox` | Download DOSBox Staging binary only |
| `pnpm test` | Run frontend tests (Vitest) |
| `pnpm run test:all` | Frontend + Rust tests |

### Regenerating the game database

The pre-built SQLite database (`metadata/exodium.db.gz`) ships with the app. To regenerate it from the bundled XML sources:

```bash
cd src-tauri
cargo run --bin generate_db
gzip -k ../metadata/exodium.db
```

---

## Project structure

```
exodium/
├── src/                    SolidJS frontend
│   ├── api/tauri.ts        Typed invoke() wrappers
│   ├── components/         GameCard, GameDetailPanel, SearchBar, WindowFrame, ...
│   ├── pages/              Intro, Setup, Library
│   └── stores/             games, downloads, thumbnails
├── src-tauri/
│   └── src/
│       ├── bin/            generate_db build tool
│       ├── commands/       Tauri commands (games, setup, updates)
│       ├── db/             SQLite schema + queries
│       ├── import/         LaunchBox XML parser
│       └── torrent/        librqbit download manager
├── metadata/               Bundled XML sources + pre-built DB (.gz)
├── scripts/
│   ├── init-dev.sh         First-time dev setup
│   └── gen_thumbnails.py   Resize + rename box art from XODOSMetadata.zip
├── manifest.json           Update-check manifest (torrent infohashes + thumbnail pack info)
├── thumbnails/eXoDOS/      Shortcode-keyed game thumbnails (gitignored, generated by init-dev)
└── torrents/               .torrent files for all 4 collections
```

---

## License

MIT - see [LICENSE](LICENSE).

Exodium does not include any game files, ROM images, or copyrighted eXoDOS assets. All game data is downloaded via the official eXoDOS torrents.
