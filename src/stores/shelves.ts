import { createSignal } from "solid-js";
import { createConfigSignal, KEY_LIST_CODEC } from "./configSignal";

/** Folded My Library shelves by shelf key (`recent`, `installed`, `pl-<id>`).
 *  The FOLDED keys are stored, so a shelf nobody touched - a new playlist -
 *  starts open. */
const stored = createConfigSignal<string[]>(
  "collapsed_shelves",
  [],
  KEY_LIST_CODEC.parse,
  KEY_LIST_CODEC.serialize,
);

/** A search opens every shelf with hits and leaves the stored state alone;
 *  folds made meanwhile live here until the search is cleared. */
const [searchFolds, setSearchFolds] = createSignal<ReadonlySet<string>>(new Set());

export const ensureShelvesLoaded = stored.ensureLoaded;
export const shelvesLoaded = stored.loaded;

export function isShelfCollapsed(key: string, searching: boolean): boolean {
  return searching ? searchFolds().has(key) : stored.value().includes(key);
}

/** Flips `key`; with `all` (Alt-click) every key in it follows to the state
 *  `key` goes to. */
export function toggleShelf(key: string, searching: boolean, all: string[] | null = null): void {
  const fold = !isShelfCollapsed(key, searching);
  const next = new Set(searching ? searchFolds() : stored.value());
  for (const k of all ?? [key]) {
    if (fold) { next.add(k); } else { next.delete(k); }
  }
  if (searching) {
    setSearchFolds(next);
    return;
  }
  // Written before the stored list arrived, this click would replace it.
  if (!stored.loaded()) { return; }
  void stored.set([...next]);
}

export function endShelfSearch(): void {
  setSearchFolds(new Set<string>());
}

/** A deleted playlist's key goes with it: SQLite hands its id to the next
 *  playlist, which must not start folded. */
export function forgetShelf(key: string): void {
  if (!stored.loaded() || !stored.value().includes(key)) { return; }
  void stored.set(stored.value().filter((k) => k !== key));
}
