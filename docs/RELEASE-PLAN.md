# Exodium v1.0 Release Plan

*Written 2026-07-12 from a 7-agent audit at v0.6.9, trimmed 2026-10-02 at v0.15.1. The original phase-by-phase checklist is in the git history of this file; what follows is what is still true.*

## Done since the audit

All five phases of the audit have shipped between 0.7 and 0.15:

- **Correctness blockers:** torrent-file matching anchored on a path boundary, a catalogue upgrade path (`CATALOG_VERSION` + `refresh_catalog`), cross-collection placeholder cleanup, fastresume hydration, the uninstall / re-download loop, startup errors as dialogs.
- **Audio:** MT-32 / SoundCanvas assets fetched from the user's own torrent, ECE config keys translated for Staging; field-verified.
- **UX and hardening:** stall feedback, empty states, single instance, disk-space preflight, asset scope and CSP, GPL staging, in-app eXoDOS attribution.
- **Seeding:** opt-in during setup and changeable in Settings, never silent.
- **CI and tests:** `ci.yml` on push and pull request (vitest, `cargo test`, typecheck, clippy), release workflow with all three installers, a Linux and a Windows lab VM for end-to-end runs.
- **eXo approval** for the redistribution surface was received on 2026-07-13.

## Still open before v1.0

1. **Code signing and notarization.** The Windows installer and the macOS app are unsigned, so both OSes show a warning on first start. Apple Developer ID and a Windows signing certificate are the gate for v1.0; donations are earmarked for them (see the README).
2. **eXoWin9x on Windows** has not been run end to end (extracted-tree resolution, npcap, the Wi-Fi probe, parent-VHD case aliases).
3. **Linux items that need an unlocked desktop session:** resolver order in the running app (pack vs system DOSBox-X), the network row in Settings, the Flatpak fallback, auto-queue plus the activity badge.
4. **Catalogue updates for new eXo releases** need the merge import (match old rows to new by shortcode, carry user state across) before any "new release available" notice makes sense. Tracked in #18.
5. **Native x86_64 Linux run of the ScummVM AppImages**, tracked in #29.

## Deferred past v1.0

From the audit's post-release list, not re-checked since: keyboard navigation for game cards and a focus trap in the detail panel.
