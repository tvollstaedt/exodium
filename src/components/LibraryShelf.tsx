import { createEffect, Show, type JSX } from "solid-js";
import { clearSelection, selectionCount, selectionScope } from "../stores/selection";
import { ensureShelvesLoaded, isShelfCollapsed, shelvesLoaded, toggleShelf } from "../stores/shelves";
import { IconChevron } from "./icons";

/** A My Library shelf, selection scope `shelf:<key>` (§25). Its header row
 *  folds it; Alt-click sets every key in `allShelves` to the same state. */
export function LibraryShelf(props: {
  shelf: string;
  title: string;
  count: number;
  allShelves: string[];
  /** A search shows every shelf open (stores/shelves.ts). */
  searching: boolean;
  /** Header controls beside the toggle, outside its hit area. */
  actions?: JSX.Element;
  children: JSX.Element;
}) {
  ensureShelvesLoaded();
  const zone = () => `shelf:${props.shelf}`;
  const collapsed = () => isShelfCollapsed(props.shelf, props.searching);

  // Nothing may stay selected that the user cannot see.
  createEffect(() => {
    if (collapsed() && selectionScope() === zone() && selectionCount() > 0) { clearSelection(); }
  });

  return (
    <div
      class="library-section"
      data-shelf-key={props.shelf}
      data-select-zone={zone()}
      data-collapsed={String(collapsed())}
    >
      <h2 class="section-title">
        <button
          class="shelf-toggle"
          data-testid="shelf-toggle"
          aria-expanded={!collapsed()}
          title={`${collapsed() ? "Show" : "Hide"} this shelf (Alt/Option-click: all shelves)`}
          onClick={(e) => toggleShelf(props.shelf, props.searching, e.altKey ? props.allShelves : null)}
        >
          <span class="shelf-chevron"><IconChevron /></span>
          {props.title}
          <span class="section-count">{props.count}</span>
        </button>
        {props.actions}
      </h2>
      {/* Not before the stored state arrived: a folded shelf must not flash open. */}
      <Show when={shelvesLoaded() && !collapsed()}>{props.children}</Show>
    </div>
  );
}
