# Handover: Playback- und Render-Probleme unter Linux (0.14.0-Draft)

Stand 2026-09-08, `main` bei `10e1b42a`, Draft-Release `v0.14.0` auf
demselben Commit (13 Assets, `latest` bleibt `v0.13.2`). Diese Session lief
auf macOS und konnte die Linux-Befunde nur mit Instrumentierung beantworten;
die Linux-Session misst.

## Bug-Liste

Kommt von Thomas in der Linux-Session selbst. Aus der macOS-Session bekannt
(Reports vom 2026-09-08, nichts davon dort reproduzierbar): Vorschau- und
Theme-Autoplay starten nicht, Pause-Knopf mit farbigem Emoji ueberlagert,
danach ein ▶ ohne Wirkung, Spinner und Cover-Zoom zittern um ein Pixel,
Kartentitel flackern beim Scrollen durch die sticky Tab-Leiste.

## Bauen und testen: lokal, nicht per AppImage

`pnpm install && pnpm run init-dev && pnpm tauri dev` - Debug-Build mit
Devtools (Rechtsklick/Inspector ist im Dev-Build nicht unterdrueckt). Das ist
der schnelle Zyklus fuer alles Logische (Store, Panel, CSS). Eine Einschraenkung
bleibt: der Dev-Build nutzt WebKitGTK und GStreamer des HOSTS, das AppImage
bringt eigenen WebKit-Core und eigene GStreamer-Plugins mit (CLAUDE.md §14).
Ein Codec-Befund gilt also erst, wenn er auch im AppImage aus dem Draft
`v0.14.0` (oder `pnpm tauri build --bundles appimage`) nachgestellt ist.

## Was seit den Reports geaendert wurde (alles in `10e1b42a` und davor, ungetestet auf Linux)

- Transport-Glyphen sind SVG (`IconPlay/Pause/Prev/Next` in `icons.tsx`) - (3)
  sollte damit weg sein.
- `musicPlayError` (Store) / `videoError` (Panel): ein verweigerter `play()`
  steht als rote Zeile in der Leiste bzw. im Hero ("Can't play this track -
  NotSupportedError: …"). `AbortError` wird ignoriert, `NotAllowedError` nennt
  ▶ als Ausweg. Der Transport folgt zusaetzlich dem `playing`-Event.
- Panel-Play auf den geladenen Track ueberstimmt jetzt Pausengruende
  (`listenerWantsSound`); vorher No-op unter laufender Vorschau - Kandidat
  fuer "▶ ohne Wirkung".
- `ended`-Flag: ein zu Ende gelaufener Track wird von `resumeFrom` nicht neu
  gestartet.
- Unmuted Video-Autoplay nimmt `pauseFor("video")` beim Planen.
- Sticky Leisten und Spinner auf eigene Compositing-Layer
  (`transform: translateZ(0)`, Block "WebKitGTK compositing" in `main.css`) -
  Hypothese fuer Zittern und Flackern, nicht gemessen.
- Play-Knopf spinnt "Preparing…" solange eine `pending`-Notiz steht.

## Was zu messen ist

1. **Autoplay / ▶ ohne Wirkung**: Steht jetzt eine Fehlerzeile? Dann
   ist der Name die Antwort:
   - `NotAllowedError` -> Autoplay-Policy. wry setzt `AutoplayPolicy::Allow`
     (wry 0.54 `webkitgtk/mod.rs:397`, Tauri fasst es nicht an); falls das
     unter dieser WebKitGTK-Version nicht greift, ist die Geste das Thema
     (2 s `VIDEO_START_DELAY_MS` nach dem Klick, WebKit vergisst die Geste).
   - `NotSupportedError` / `MediaError 3|4` -> Decoder/Quelle. Pruefen:
     `GST_DEBUG=3 ./Exodium.AppImage 2>&1 | grep -i -E "error|warn"`,
     welcher Container (mp3/ogg, H.264) betroffen ist, ob nur die
     gebuendelten Plugins (`bundleMediaFramework`, CLAUDE.md §14) den Codec
     nicht haben. Auf Arch-artigen Hosts: `gst-inspect-1.0 | grep -i
     -E "mpg123|avdec_h264|vorbis"` gegen die Liste im AppDir.
   - Keine Zeile, ▶ ohne Wirkung -> `play()` schwebt. Dann `playing`-Event
     pruefen (WebKit-Inspector: `document.querySelector("audio").readyState`,
     `networkState`, `error`).
   Inspector: im Dev-Build direkt; fuer das AppImage
   `WEBKIT_INSPECTOR_SERVER=127.0.0.1:9222` setzen und die URL im Browser
   oeffnen (nur wenn der Build das Devtools-Feature hat).
2. **Theme startet nicht, ohne Fehlerzeile**: Wurde vorher Pause oder × geklickt? Dann ist es
   die Session-Sperre (`musicUserPaused`/`playerHidden`, Entscheidung
   2026-09-06 in DECISIONS.md) und kein Bug.
3. **Zittern und Flackern**: Mit dem aktuellen Stand erneut schauen. Bleibt das Cover-Zittern:
   Hypothese ist Pixel-Snapping der Layer-Position bei 1x DPI waehrend der
   `scale`-Transition (auf Retina unsichtbar). Vor einem Fix messen, ob
   `.game-card { will-change: transform }` (nur fuer die gehoverte Karte,
   z.B. per `:hover` reicht nicht - Layer muss vor der Transition existieren)
   es aendert, und was es bei ~500 Karten im DOM an Speicher kostet.
   Render-Pfad pruefen: `ls -l /proc/$(pgrep -x exodium)/fd | grep -c nvidia`
   (7 = Software-Rasterizer, ~38 = GPU, CLAUDE.md §17).

## Regeln fuer diese Session

- Antworten auf Deutsch, Commits 1-2 Zeilen, keine Spielzahlen in UI-Text.
- Vor jedem `git push` fragen. `v0.14.0` bleibt Draft; `latest` ist `v0.13.2`.
  Draft neu bauen = Tag `v0.14.0` force-moven, Release loeschen, CI baut.
- Content-Releases immer `--latest=false`.
- Was gemessen wurde, in `docs/DECISIONS.md` (max. 5 Zeilen) und ggf.
  CLAUDE.md §14/§17 festhalten; oeffentliche Texte nennen keine §/CLAUDE.md.
- Vor dem Push: `pnpm test`, `pnpm run typecheck`, bei Rust
  `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`.
