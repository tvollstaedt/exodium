/**
 * Line icons for the detail panel, in the same shape as ActivityBadge's:
 * 24 viewBox, `currentColor`, round joins.
 *
 * They are components rather than emoji because an emoji renders in the
 * system's own colour font - it would sit next to ⚙ / ↺ / ▶ as the one glyph
 * whose colour the theme has no say over.
 */

export const IconSoundOn = () => (
  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
    <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" fill="currentColor" stroke-linejoin="round" />
    <path stroke-linecap="round" d="M15.54 8.46a5 5 0 0 1 0 7.07" />
    <path stroke-linecap="round" d="M19.07 4.93a10 10 0 0 1 0 14.14" />
  </svg>
);

export const IconSoundOff = () => (
  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
    <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" fill="currentColor" stroke-linejoin="round" />
    <path stroke-linecap="round" d="M22 9l-6 6m0-6l6 6" />
  </svg>
);

export const IconAutoplay = () => (
  <svg
    width="15"
    height="15"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <path d="M17 2l4 4-4 4" />
    <path d="M3 11V9a4 4 0 0 1 4-4h14" />
    <path d="M7 22l-4-4 4-4" />
    <path d="M21 13v2a4 4 0 0 1-4 4H3" />
  </svg>
);

export const IconShuffle = () => (
  <svg
    width="15"
    height="15"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <path d="M16 3h5v5" />
    <path d="M4 20 21 3" />
    <path d="M21 16v5h-5" />
    <path d="M15 15l6 6" />
    <path d="M4 4l5 5" />
  </svg>
);

export const IconMusicNote = () => (
  <svg
    width="15"
    height="15"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="2"
    stroke-linecap="round"
    stroke-linejoin="round"
  >
    <path d="M9 18V5l12-2v13" />
    <circle cx="6" cy="18" r="3" />
    <circle cx="18" cy="16" r="3" />
  </svg>
);

export const IconZoom = () => (
  <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
    <circle cx="11" cy="11" r="7" />
    <path stroke-linecap="round" d="M20.5 20.5 16 16M11 8v6M8 11h6" />
  </svg>
);

/**
 * One pictogram per metadata field.
 *
 * They are an ANCHOR, not a replacement for the label: "Developer" and
 * "Publisher" have no pictogram anyone would read correctly, and a column of
 * unlabelled glyphs would trade a text-heavy block for a guessing game.
 */
const FIELD_ICONS = {
  platform: "M2 4h20v13H2zM8 21h8M12 17v4",
  emulator: "M6 6h12v12H6zM10 10h4v4h-4zM10 2v4M14 2v4M10 18v4M14 18v4M2 10h4M2 14h4M18 10h4M18 14h4",
  developer: "M16 18l6-6-6-6M8 6l-6 6 6 6",
  publisher: "M21 8v8l-9 5-9-5V8l9-5 9 5zM3 8l9 5 9-5M12 13v9",
  series: "M12 2 2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5",
  genre: "M20.6 13.4 13.4 20.6a2 2 0 0 1-2.8 0L2 12V2h10l8.6 8.6a2 2 0 0 1 0 2.8zM7 7h.01",
  mode: "M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2M9 3a4 4 0 1 1 0 8 4 4 0 0 1 0-8M23 21v-2a4 4 0 0 0-3-3.9M16 3.1a4 4 0 0 1 0 7.8",
  region: "M12 2a10 10 0 1 1 0 20 10 10 0 0 1 0-20zM2 12h20M12 2a15 15 0 0 1 4 10 15 15 0 0 1-4 10 15 15 0 0 1-4-10 15 15 0 0 1 4-10z",
  players: "M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2M12 3a4 4 0 1 1 0 8 4 4 0 0 1 0-8",
  rating: "M12 2.5l2.9 5.9 6.5.9-4.7 4.6 1.1 6.5-5.8-3-5.8 3 1.1-6.5L2.6 9.3l6.5-.9z",
  year: "M3 5h18v16H3zM3 10h18M8 3v4M16 3v4",
} as const;

/** Union of the known field icons. A plain `string` would let a typo through
 *  as `<path d={undefined}>`, which renders nothing and reports nothing. */
export type FieldIconName = keyof typeof FIELD_ICONS;

export const FieldIcon = (props: { name: FieldIconName }) => (
  <svg
    class="game-detail-field-icon"
    width="13"
    height="13"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.8"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <path d={FIELD_ICONS[props.name]} />
  </svg>
);
