import { createSignal } from "solid-js";
import type { Game, GameRef } from "../api/tauri";
import { parseLangEntries } from "../util";

/** Multi-select over ONE list at a time (§25): the Browse grid/list is scope
 *  "browse", a My Library shelf `shelf:<key>`. Picking in another list starts
 *  over - a selection spanning shelves would act on rows nobody can see. */
const [scope, setScope] = createSignal<string | null>(null);
const [items, setItems] = createSignal<ReadonlyMap<number, GameRef>>(new Map());
const [selectMode, setSelectMode] = createSignal(false);
/** Where a Shift-click range starts: the last card picked on its own. */
let anchor: number | null = null;

export { scope as selectionScope, selectMode, setSelectMode };
export const selectionCount = () => items().size;
export const selectedRefs = () => [...items().values()];
export const isSelected = (s: string | undefined, id: number | null | undefined) =>
  s != null && id != null && scope() === s && items().has(id);

/** A rendered card as the selection keeps it; the group's state comes from
 *  its language map, since the card's own row may be the uninstalled one. */
export function refOf(g: Game): GameRef | null {
  if (g.id == null) { return null; }
  const entries = parseLangEntries(g);
  return {
    id: g.id,
    title: g.title,
    torrent_source: g.torrent_source ?? null,
    thumbnail_key: g.thumbnail_key ?? null,
    installed: g.installed || entries.some((e) => e.state === 2),
    in_library: g.in_library || entries.some((e) => e.state >= 1),
    favorited: g.favorited,
  };
}

function enter(s: string): Map<number, GameRef> {
  if (scope() !== s) {
    setScope(s);
    anchor = null;
    return new Map();
  }
  return new Map(items());
}

export function toggleSelected(s: string, g: Game) {
  const ref = refOf(g);
  if (!ref) { return; }
  const next = enter(s);
  if (next.has(ref.id)) { next.delete(ref.id); } else { next.set(ref.id, ref); }
  anchor = ref.id;
  setItems(next);
}

/** Shift-click: everything from the anchor to `g` in `ordered`, added to what
 *  is selected. Without an anchor in this list it is a plain toggle. */
export function selectRange(s: string, ordered: Game[], g: Game) {
  const from = anchor == null ? -1 : ordered.findIndex((x) => x.id === anchor);
  const to = ordered.findIndex((x) => x.id === g.id);
  if (scope() !== s || from < 0 || to < 0) {
    toggleSelected(s, g);
    return;
  }
  const next = enter(s);
  for (const x of ordered.slice(Math.min(from, to), Math.max(from, to) + 1)) {
    const ref = refOf(x);
    if (ref) { next.set(ref.id, ref); }
  }
  setItems(next);
}

/** Marquee and "Select all": `additive` keeps what was selected before. */
export function selectRefs(s: string, refs: GameRef[], additive = false) {
  const next = additive ? enter(s) : new Map<number, GameRef>();
  if (!additive) {
    setScope(s);
    anchor = null;
  }
  for (const r of refs) { next.set(r.id, r); }
  setItems(next);
}

export function clearSelection() {
  setItems(new Map());
  anchor = null;
}

export function exitSelection() {
  clearSelection();
  setSelectMode(false);
}

/** Bulk favorite changes for the cards, which track their flag themselves. */
const [favoriteChange, setFavoriteChange] = createSignal<{ ids: ReadonlySet<number>; value: boolean } | null>(null);
export { favoriteChange };

export function announceFavorites(ids: number[], value: boolean) {
  setFavoriteChange({ ids: new Set(ids), value });
  setItems((prev) => new Map([...prev].map(([id, r]) => [id, ids.includes(id) ? { ...r, favorited: value } : r])));
}
