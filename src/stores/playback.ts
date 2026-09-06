import { createConfigSignal, BOOL_CODEC } from "./configSignal";

/** Preview mute, global and persistent; defaults to unmuted. */
const muted = createConfigSignal<boolean>(
  "preview_muted",
  false,
  BOOL_CODEC.parse,
  BOOL_CODEC.serialize,
);

export const ensurePreviewMutedLoaded = muted.ensureLoaded;
export const previewMuted = muted.value;
export const setPreviewMuted = muted.set;
