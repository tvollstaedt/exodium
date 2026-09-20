import { createSignal, createEffect, on, onMount, onCleanup, type Accessor } from "solid-js";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { Game } from "../api/tauri";
import { thumbnailCandidates } from "../stores/thumbnails";
import { observeNearViewport, unobserveNearViewport } from "../nearViewport";

/** A cover for a tile, by source and content key: resolved once the element
 *  is within ~2 screens of the viewport (nearViewport.ts), walking the tier
 *  candidates on each <img onError> (a stale poster dir 404s through to the
 *  bundled preview). `src` is null before that and once every candidate
 *  failed. `identity` restarts the walk when the tile shows something else.  */
export function createKeyedCover(
  source: Accessor<string | null | undefined>,
  key: Accessor<string | null | undefined>,
  identity: Accessor<string>,
  el: () => HTMLElement | undefined,
) {
  const [near, setNear] = createSignal(false);
  const [idx, setIdx] = createSignal(0);
  const [exhausted, setExhausted] = createSignal(false);

  onMount(() => {
    const node = el();
    if (node) { observeNearViewport(node, () => setNear(true)); }
  });
  onCleanup(() => {
    const node = el();
    if (node) { unobserveNearViewport(node); }
  });

  const candidates = () => thumbnailCandidates(source(), key());

  // A new subject OR a changed tier list restarts the walk (a removed pack
  // shortens the list under a tile's index).
  createEffect(on(
    () => `${identity()}|${candidates().join("|")}`,
    () => { setExhausted(false); setIdx(0); },
    { defer: true },
  ));

  const src = () => {
    if (!near() || exhausted()) { return null; }
    const c = candidates();
    // Clamp rather than trust the index: the reset effect lands a frame
    // later, and an out-of-range index would unmount the <img> for that frame.
    const path = c[idx()] ?? c[c.length - 1];
    return path ? convertFileSrc(path) : null;
  };

  const onError = () => {
    if (idx() + 1 < candidates().length) { setIdx(idx() + 1); } else { setExhausted(true); }
  };

  return { src, onError };
}

export function createCover(game: Accessor<Game>, el: () => HTMLElement | undefined) {
  return createKeyedCover(
    () => game().torrent_source,
    () => game().thumbnail_key,
    () => String(game().id),
    el,
  );
}
