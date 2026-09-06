import { For, Show } from "solid-js";
import exodosCover from "../assets/collections/exodos.jpg";
import glpCover from "../assets/collections/glp.jpg";
import plpCover from "../assets/collections/plp.jpg";
import slpCover from "../assets/collections/slp.jpg";
import exowin3xCover from "../assets/collections/exowin3x.jpg";
import exowin9xCover from "../assets/collections/exowin9x.jpg";
import exoscummvmCover from "../assets/collections/exoscummvm.jpg";

export interface ShelfCollection {
  id: string;
  label: string;
  count: number;
  /** Overrides the "<count> games" line when set (the All card shows
   *  "<n> collections" - a row-count sum would double-count LP variants). */
  sub?: string;
}

interface Props {
  collections: ShelfCollection[];
  active: string;
  onSelect: (id: string) => void;
}

/** eXo's official box art per collection (retro-exo.com section covers),
 *  bundled at 132px - the shelf renders them 62px wide. */
const COVER_ART: Record<string, string> = {
  eXoDOS: exodosCover,
  eXoDOS_GLP: glpCover,
  eXoDOS_PLP: plpCover,
  eXoDOS_SLP: slpCover,
  eXoWin3x: exowin3xCover,
  eXoWin9x: exowin9xCover,
  eXoScummVM: exoscummvmCover,
};

/** Dominant box color per collection - drives the card's ambient glow and the
 *  active ring, so each collection lights up in its own tint. */
const ACCENT: Record<string, string> = {
  eXoDOS: "#c19a5f",
  eXoDOS_GLP: "#d23c2a",
  eXoDOS_PLP: "#d04338",
  eXoDOS_SLP: "#4a6bd6",
  eXoWin3x: "#e0442e",
  eXoWin9x: "#c99a45",
  eXoScummVM: "#3d9a4e",
};

/** Card titles: the shelf shows the count right below, so "German Language
 *  Pack" carries no information "German" doesn't. */
const shortLabel = (label: string) => label.replace(" Language Pack", "");

/** The "All" card (empty id = backend's no-collection-filter) gets a 2x2
 *  mosaic of the shelf's own covers instead of a single box. */
const ALL_MOSAIC = [exodosCover, glpCover, slpCover, exowin3xCover];

const cover = (col: ShelfCollection) => {
  if (col.id === "") {
    return (
      <span class="collection-cover collection-cover-all">
        <For each={ALL_MOSAIC}>{(src) => <img src={src} alt="" draggable={false} />}</For>
      </span>
    );
  }
  return (
    <Show
      when={COVER_ART[col.id]}
      fallback={
        <span class="collection-cover collection-cover-fallback">
          {shortLabel(col.label).slice(0, 2)}
        </span>
      }
    >
      <img class="collection-cover" src={COVER_ART[col.id]} alt="" draggable={false} />
    </Show>
  );
};

/** Horizontal rail of collection boxes above the Browse grid - one card per
 *  collection: eXo's box art, name, game count. The active box is lit in its
 *  own accent color and lifted off the shelf; a future collection without
 *  bundled art falls back to an initials box in the accent tint. */
export function CollectionShelf(props: Props) {
  return (
    <div class="collection-shelf" role="group" aria-label="Collections">
      <For each={props.collections}>
        {(col) => (
          <button
            class={`collection-card ${props.active === col.id ? "active" : ""}`}
            style={{ "--card-accent": ACCENT[col.id] ?? "#7c5cfc" }}
            onClick={() => props.onSelect(col.id)}
            title={col.label}
          >
            <span class="collection-cover-wrap">{cover(col)}</span>
            <span class="collection-card-name">{shortLabel(col.label)}</span>
            <span class="collection-card-count">
              <Show when={!col.sub} fallback={col.sub}>
                <b>{col.count.toLocaleString()}</b> games
              </Show>
            </span>
          </button>
        )}
      </For>
    </div>
  );
}
