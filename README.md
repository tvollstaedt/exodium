# Exodian

A cross-platform game launcher for the [eXoDOS](https://www.retro-exo.com/exodos.html) collection. Replaces the Windows-only LaunchBox frontend with a native desktop app that runs on Linux, macOS, and Windows.

## Features

- Browse 9,198 games across 4 collections (eXoDOS, German, Spanish, Polish)
- Stream individual games from the eXoDOS torrents on demand (no full 500 GB download needed)
- Launch games via DOSBox Staging with auto-patched configs
- Collection tabs for browsing each language pack separately
- Shortcode-keyed thumbnails for box art
- Save game backup/restore on uninstall

## Tech Stack

| Layer | Technology |
|-------|-----------|
| Shell | Tauri v2 |
| Frontend | SolidJS + TypeScript + Vite |
| UI Components | Ark UI (`@ark-ui/solid`) |
| Backend | Rust |
| Database | SQLite (pre-built, ships with the app) |
| Torrent | librqbit (selective file downloads) |

## Development

```bash
pnpm install
pnpm tauri dev
```

Requires DOSBox Staging installed for game launching.

## Regenerating the Game Database

The pre-built SQLite database (`metadata/exodian.db.gz`) ships with the app and contains all game metadata, torrent indices, and shortcode mappings. To regenerate it from the source XML files:

```bash
cd src-tauri
cargo run --bin generate_db
gzip -k ../metadata/exodian.db
```

## Project Structure

```
exodian/
├── src/                    SolidJS frontend
│   ├── api/tauri.ts        Typed invoke() wrappers
│   ├── components/         GameCard, SearchBar, Select, WindowFrame
│   ├── pages/              Setup, Library
│   └── stores/             games, downloads, thumbnails
├── src-tauri/
│   └── src/
│       ├── bin/            generate_db build tool
│       ├── commands/       Tauri commands (games, setup)
│       ├── db/             SQLite schema + queries
│       ├── import/         LaunchBox XML parser
│       └── torrent/        librqbit download manager
├── metadata/               Bundled XML sources + pre-built DB
├── thumbnails/eXoDOS/      Shortcode-keyed game thumbnails
└── torrents/               .torrent files for all 4 collections
```

## License

MIT
