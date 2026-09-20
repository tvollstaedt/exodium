import type { ViewMode } from "../stores/view";

/** The grid/list switch, one instance per toolbar so it sits in the same
 *  right-edge spot on every tab. The caller owns the fallback for a sort the
 *  target mode cannot show. */
export function ViewToggle(props: { mode: ViewMode; onChange: (mode: ViewMode) => void }) {
  return (
    <div class="view-toggle" role="group" aria-label="View mode" data-testid="view-toggle">
      <button
        class={`view-toggle-btn ${props.mode === "grid" ? "active" : ""}`}
        title="Grid view"
        data-testid="view-grid"
        onClick={() => props.onChange("grid")}
      >▦</button>
      <button
        class={`view-toggle-btn ${props.mode === "list" ? "active" : ""}`}
        title="List view"
        data-testid="view-list"
        onClick={() => props.onChange("list")}
      >☰</button>
    </div>
  );
}
