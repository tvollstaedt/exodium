/** Width rules of the detail panel (§12), pure so vitest covers them. The
 *  width is stored as a FRACTION of the window: a pixel value would come back
 *  wrong after the window is resized or the app opens on another display. */

export const PANEL_MIN_PX = 380;
/** What stays visible of the grid beside a panel dragged to its widest. */
export const GRID_MIN_PX = 240;
export const SNAP_FRACTIONS = [1 / 3, 1 / 2, 2 / 3];
/** Distance to a snap point, as a fraction of the window, that still snaps. */
export const SNAP_TOLERANCE = 0.02;

export function parseWidthFraction(raw: string | null): number | null {
  if (raw == null || raw === "") { return null; }
  const f = Number(raw);
  return Number.isFinite(f) && f > 0 && f < 1 ? f : null;
}

/** Null (the default width) is stored as the empty string. */
export function serializeWidthFraction(f: number | null): string {
  return f == null ? "" : f.toFixed(4);
}

export function snapFraction(f: number): number {
  for (const s of SNAP_FRACTIONS) {
    if (Math.abs(f - s) <= SNAP_TOLERANCE) { return s; }
  }
  return f;
}

/** The pixel width a fraction yields in this window, kept between the
 *  panel's minimum and the window minus the grid margin. */
export function clampPanelWidth(fraction: number, windowWidth: number): number {
  const max = Math.max(PANEL_MIN_PX, windowWidth - GRID_MIN_PX);
  return Math.round(Math.min(max, Math.max(PANEL_MIN_PX, fraction * windowWidth)));
}

/** The fraction a pointer at `clientX` asks for; the panel hangs on the right edge. */
export function fractionFromPointer(clientX: number, windowWidth: number): number {
  if (windowWidth <= 0) { return 0; }
  return Math.min(1, Math.max(0, (windowWidth - clientX) / windowWidth));
}
