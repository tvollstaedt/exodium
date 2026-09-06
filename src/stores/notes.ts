import { createConfigSignal, KEY_LIST_CODEC } from "./configSignal";

/** Dismissed panel notes, keyed by KIND (the ECE note is the same sentence
 *  on ~2,000 titles). */
const dismissed = createConfigSignal<string[]>(
  "dismissed_notes",
  [],
  KEY_LIST_CODEC.parse,
  KEY_LIST_CODEC.serialize,
);

export const ensureDismissedNotesLoaded = dismissed.ensureLoaded;

/** False until the stored list has arrived, so a note the user silenced weeks
 *  ago does not flash on the first panel open of a session. */
export const dismissedNotesLoaded = dismissed.loaded;

export function isNoteDismissed(key: string): boolean {
  return dismissed.value().includes(key);
}

export async function dismissNote(key: string): Promise<void> {
  await dismissed.set([...dismissed.value(), key]);
}
