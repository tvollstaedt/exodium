/** Canvas-painted video for Linux (§14 "rock-solid playback").
 *
 *  WebKitGTK's own on-screen video path is not trustworthy across GPU
 *  stacks: on NVIDIA it showed green or stale frames while decode,
 *  `requestVideoFrameCallback` and `drawImage` were all correct (measured
 *  2026-09-08: rVFC at the file's frame rate, moving and correctly colored
 *  pixels out of `drawImage`, screen green regardless of decoder). So on
 *  Linux the app paints the frames itself: a canvas laid over the element
 *  mirrors it frame by frame, and the element stays the audio source, the
 *  clock and the click target. Everywhere else this module is a no-op.
 */
import { createSignal } from "solid-js";
import { videoMirrorNeeded } from "./api/tauri";

/** Whether this machine needs the mirror: Linux with the proprietary NVIDIA
 *  driver, answered by the backend (the same predicate that disables the
 *  DMABuf sink at startup). Everywhere else - WKWebView, WebView2, and Linux
 *  on Mesa, where the zero-copy sink works and readback might not - the
 *  engine presents the video itself and the mirror stays out of the way. */
const [mirrorNeeded, setMirrorNeeded] = createSignal(false);
let asked: Promise<void> | null = null;

export function ensureVideoMirrorKnown(): void {
  asked ??= videoMirrorNeeded()
    .then((needed) => { setMirrorNeeded(needed === true); })
    .catch(() => {});
}

export const needsCanvasVideo = mirrorNeeded;

/** Source/destination rectangles that emulate `object-fit: cover`. */
export function coverRect(vw: number, vh: number, cw: number, ch: number) {
  const scale = Math.max(cw / vw, ch / vh);
  const sw = cw / scale;
  const sh = ch / scale;
  return { sx: (vw - sw) / 2, sy: (vh - sh) / 2, sw, sh, dx: 0, dy: 0, dw: cw, dh: ch };
}

/** Same for `object-fit: contain` (letterboxed, background left alone). */
export function containRect(vw: number, vh: number, cw: number, ch: number) {
  const scale = Math.min(cw / vw, ch / vh);
  const dw = vw * scale;
  const dh = vh * scale;
  return { sx: 0, sy: 0, sw: vw, sh: vh, dx: (cw - dw) / 2, dy: (ch - dh) / 2, dw, dh };
}

type RvfcVideo = HTMLVideoElement & {
  requestVideoFrameCallback?: (cb: () => void) => number;
  cancelVideoFrameCallback?: (handle: number) => void;
};

/** Mirror `video` into `canvas` until the returned dispose runs. Paints on
 *  every decoded frame (rVFC; rAF while playing as the fallback) and once per
 *  load/seek so a paused element still shows its frame. */
export function attachCanvasPainter(
  video: HTMLVideoElement,
  canvas: HTMLCanvasElement,
  fit: "cover" | "contain",
): () => void {
  const ctx = canvas.getContext("2d");
  if (!ctx) { return () => {}; }
  let disposed = false;
  let raf = 0;

  const paint = () => {
    const vw = video.videoWidth;
    const vh = video.videoHeight;
    const cw = canvas.clientWidth;
    const ch = canvas.clientHeight;
    if (!vw || !vh || !cw || !ch) { return; }
    // Backing store at device resolution, or the mirror is the blurry copy.
    const dpr = window.devicePixelRatio || 1;
    const bw = Math.round(cw * dpr);
    const bh = Math.round(ch * dpr);
    if (canvas.width !== bw || canvas.height !== bh) {
      canvas.width = bw;
      canvas.height = bh;
    }
    const r = fit === "cover" ? coverRect(vw, vh, bw, bh) : containRect(vw, vh, bw, bh);
    if (fit === "contain") { ctx.clearRect(0, 0, bw, bh); }
    try {
      ctx.drawImage(video, r.sx, r.sy, r.sw, r.sh, r.dx, r.dy, r.dw, r.dh);
    } catch {
      // A frame that cannot be drawn (source not ready) is skipped, not fatal.
    }
  };

  const rvfc = video as RvfcVideo;
  const armRvfc = () => {
    if (disposed) { return; }
    rvfc.requestVideoFrameCallback!(() => { paint(); armRvfc(); });
  };
  const armRaf = () => {
    if (disposed) { return; }
    paint();
    raf = requestAnimationFrame(armRaf);
  };

  if (typeof rvfc.requestVideoFrameCallback === "function") {
    armRvfc();
  } else {
    armRaf();
  }
  // A paused element still owes its current frame (first open, seek).
  video.addEventListener("loadeddata", paint);
  video.addEventListener("seeked", paint);

  return () => {
    disposed = true;
    if (raf) { cancelAnimationFrame(raf); }
    video.removeEventListener("loadeddata", paint);
    video.removeEventListener("seeked", paint);
  };
}
