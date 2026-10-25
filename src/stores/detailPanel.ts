import { createConfigSignal, BOOL_CODEC } from "./configSignal";
import { parseWidthFraction, serializeWidthFraction } from "../detailPanelLayout";

/** The detail panel's width as a fraction of the window; null = the CSS default. */
const width = createConfigSignal<number | null>(
  "detail_panel_width",
  null,
  parseWidthFraction,
  serializeWidthFraction,
);
export const panelWidthFraction = width.value;
export const setPanelWidthFraction = width.set;

const fullscreen = createConfigSignal<boolean>(
  "detail_panel_fullscreen",
  false,
  BOOL_CODEC.parse,
  BOOL_CODEC.serialize,
);
export const panelFullscreen = fullscreen.value;
export const setPanelFullscreen = fullscreen.set;

export function ensureDetailPanelPrefsLoaded(): void {
  width.ensureLoaded();
  fullscreen.ensureLoaded();
}
