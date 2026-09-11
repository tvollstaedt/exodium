import { createSignal, createEffect, on, onCleanup, Show, For } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { convertFileSrc } from "@tauri-apps/api/core";
import { previewMuted } from "../stores/playback";
import { attachCanvasPainter, ensureVideoMirrorKnown, needsCanvasVideo } from "../videoCanvas";

interface LightboxProps {
  images: string[];
  /** Preview video, shown as the first entry when present. Already an asset
   *  URL - unlike `images`, which are filesystem paths. */
  video?: string | null;
  /** The trailer is the entry currently shown (it autoplays with sound). */
  onVideoShown?: (shown: boolean) => void;
  startIndex: number;
  open: boolean;
  onClose: () => void;
}

const ZOOM_SCALE = 2.5;

export function Lightbox(props: LightboxProps) {
  const [idx, setIdx] = createSignal(0);
  const [zoomed, setZoomed] = createSignal(false);
  const [panX, setPanX] = createSignal(0);
  const [panY, setPanY] = createSignal(0);
  const [imgLoadError, setImgLoadError] = createSignal(false);
  let stageRef: HTMLDivElement | undefined;
  let lbVideoRef: HTMLVideoElement | undefined;
  ensureVideoMirrorKnown();

  const resetZoom = () => { setZoomed(false); setPanX(0); setPanY(0); setImgLoadError(false); };

  const hasVideo = () => !!props.video;
  // The hero's speaker claim follows what is on screen: the user walks to
  // and from the trailer with the arrows and the strip.
  createEffect(() => props.onVideoShown?.(isVideoAt(idx())));
  const videoIndex = 0;
  const isVideoAt = (i: number) => hasVideo() && i === videoIndex;
  const imageAt = (i: number) => props.images[hasVideo() ? i - 1 : i];
  const count = () => props.images.length + (hasVideo() ? 1 : 0);

  const clampIdx = (i: number) => Math.max(0, Math.min(i, count() - 1));

  // Jump to the start entry on open only; count() changes while open.
  createEffect(on(() => props.open, (open) => {
    if (open) {
      setIdx(clampIdx(props.startIndex));
      resetZoom();
    }
  }));

  // A video landing shifts every image right by one; follow it.
  createEffect((had: boolean) => {
    const has = hasVideo();
    if (has !== had && props.open) {
      setIdx((i) => clampIdx(has ? i + 1 : i - 1));
    }
    return has;
  }, !!props.video);

  createEffect(on(() => idx(), resetZoom, { defer: true }));

  const prev = () => setIdx((i) => (i - 1 + count()) % count());
  const next = () => setIdx((i) => (i + 1) % count());

  createEffect(() => {
    if (!props.open) { return; }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "ArrowLeft") { prev(); }
      else if (e.key === "ArrowRight") { next(); }
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });

  // Pointer swipe (fit mode only).
  let swipeStartX: number | null = null;
  let didSwipe = false;
  const SWIPE_THRESHOLD = 50;
  const onPointerDown = (e: PointerEvent) => {
    if (zoomed()) { return; }
    swipeStartX = e.clientX;
    didSwipe = false;
  };
  const onPointerUp = (e: PointerEvent) => {
    if (zoomed() || swipeStartX == null) { return; }
    const dx = e.clientX - swipeStartX;
    swipeStartX = null;
    if (dx <= -SWIPE_THRESHOLD) { next(); didSwipe = true; }
    else if (dx >= SWIPE_THRESHOLD) { prev(); didSwipe = true; }
  };

  const onStageClick = (e: MouseEvent) => {
    e.stopPropagation();
    if (didSwipe) { didSwipe = false; return; }
    if ((e.target as HTMLElement).tagName === "IMG") {
      if (zoomed()) {
        resetZoom();
      } else {
        setZoomed(true);
      }
    } else {
      props.onClose();
    }
  };

  const onMouseMove = (e: MouseEvent) => {
    if (!zoomed() || !stageRef) { return; }
    const img = stageRef.querySelector("img");
    if (!img) { return; }
    const sr = stageRef.getBoundingClientRect();
    // clientWidth/Height = fitted (pre-transform) image size.
    const iw = img.clientWidth;
    const ih = img.clientHeight;
    // Pan range = how far the zoomed image overflows the stage on each side.
    // If the zoomed image is smaller than the stage in one dimension, no pan.
    const maxPanX = Math.max(0, (iw * ZOOM_SCALE - sr.width) / 2);
    const maxPanY = Math.max(0, (ih * ZOOM_SCALE - sr.height) / 2);
    // Cursor position normalized across the full stage (0..1).
    const nx = Math.max(0, Math.min(1, (e.clientX - sr.left) / sr.width));
    const ny = Math.max(0, Math.min(1, (e.clientY - sr.top) / sr.height));
    setPanX((nx - 0.5) * 2 * maxPanX);
    setPanY((ny - 0.5) * 2 * maxPanY);
  };

  const srcAt = (i: number) => {
    const path = imageAt(i);
    return path ? convertFileSrc(path) : null;
  };

  const imgTransform = () => {
    if (!zoomed()) { return undefined; }
    return `scale(${ZOOM_SCALE}) translate(${-panX() / ZOOM_SCALE}px, ${-panY() / ZOOM_SCALE}px)`;
  };

  return (
    <Show when={props.open}>
    <Dialog.Root
      open={props.open}
      onOpenChange={(e) => { if (!e.open) { props.onClose(); } }}
    >
      <Portal>
        <Dialog.Backdrop class="lightbox-backdrop" onClick={props.onClose} />
        <Dialog.Positioner class="lightbox-positioner">
          <Dialog.Content class="lightbox-content">

          <Show when={count() > 1}>
            <button class="lightbox-nav lightbox-prev" onClick={prev} title="Previous (←)">‹</button>
            <button class="lightbox-nav lightbox-next" onClick={next} title="Next (→)">›</button>
          </Show>

          <div
            ref={stageRef}
            class={`lightbox-stage ${zoomed() ? "zoomed" : ""} ${isVideoAt(idx()) ? "has-video" : ""}`}
            onPointerDown={onPointerDown}
            onPointerUp={onPointerUp}
            onClick={onStageClick}
            onMouseMove={onMouseMove}
          >
            <Show when={isVideoAt(idx())}>
              {/* Same global preference as the hero preview - opening the
                  lightbox is a bigger view of the same trailer, not a reason
                  to start making noise. Its own controls can override. */}
              <div class="lightbox-video-box" onClick={(e) => e.stopPropagation()}>
                {/* Linux paints the frames itself and drops the native
                    controls (they would sit under the canvas, videoCanvas.ts);
                    a click on the mirror toggles playback instead. */}
                <video
                  ref={(el) => {
                    lbVideoRef = el;
                    // Same rule as the hero video: a detached element keeps
                    // playing audio unless paused on unmount.
                    onCleanup(() => el.pause());
                  }}
                  class={`lightbox-video${needsCanvasVideo() ? " has-canvas" : ""}`}
                  src={props.video!}
                  controls={!needsCanvasVideo()}
                  autoplay
                  muted={previewMuted()}
                  playsinline
                />
                <Show when={needsCanvasVideo()}>
                  <canvas
                    ref={(c) => {
                      if (lbVideoRef) {
                        onCleanup(attachCanvasPainter(lbVideoRef, c, "contain"));
                      }
                    }}
                    class="lightbox-video-canvas"
                    onClick={() => {
                      const el = lbVideoRef;
                      if (!el) { return; }
                      if (el.paused) { void el.play(); } else { el.pause(); }
                    }}
                  />
                </Show>
              </div>
            </Show>
            <Show when={!isVideoAt(idx()) && srcAt(idx()) && !imgLoadError()} fallback={
              <Show when={!isVideoAt(idx())}>
                <div class="lightbox-broken">Image unavailable</div>
              </Show>
            }>
              <img
                class={`lightbox-image ${zoomed() ? "zoomed" : ""}`}
                src={srcAt(idx())!}
                alt=""
                draggable={false}
                style={{ transform: imgTransform() }}
                onError={() => setImgLoadError(true)}
              />
            </Show>
          </div>

          <Show when={count() > 1}>
            <div class="lightbox-preload" aria-hidden="true">
              <For each={[idx() - 1, idx() + 1].map((i) => (i + count()) % count())}>
                {(i) => <Show when={!isVideoAt(i) && srcAt(i)}><img src={srcAt(i)!} alt="" /></Show>}
              </For>
            </div>
          </Show>

          <Show when={count() > 1}>
            <div class="lightbox-counter">{idx() + 1} / {count()}</div>
            <div class="lightbox-thumbs">
              <Show when={hasVideo()}>
                <button
                  class={`lightbox-thumb-item lightbox-thumb-video ${idx() === videoIndex ? "active" : ""}`}
                  title="Preview video"
                  onClick={(e) => { e.stopPropagation(); setIdx(videoIndex); }}
                >▶</button>
              </Show>
              <For each={props.images}>
                {(path, i) => {
                  const at = () => (hasVideo() ? i() + 1 : i());
                  return (
                    <img
                      src={convertFileSrc(path)}
                      class={`lightbox-thumb-item ${at() === idx() ? "active" : ""}`}
                      alt=""
                      loading="lazy"
                      onClick={(e) => { e.stopPropagation(); setIdx(at()); }}
                    />
                  );
                }}
              </For>
            </div>
          </Show>
          </Dialog.Content>

        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
    </Show>
  );
}
