import { createSignal, createEffect, on, onCleanup, Show, For } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { convertFileSrc } from "@tauri-apps/api/core";

interface LightboxProps {
  images: string[];
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

  const resetZoom = () => { setZoomed(false); setPanX(0); setPanY(0); setImgLoadError(false); };

  createEffect(() => {
    if (props.open) {
      setIdx(Math.max(0, Math.min(props.startIndex, props.images.length - 1)));
      resetZoom();
    }
  });

  createEffect(on(() => idx(), resetZoom, { defer: true }));

  const count = () => props.images.length;
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
    const path = props.images[i];
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
            class={`lightbox-stage ${zoomed() ? "zoomed" : ""}`}
            onPointerDown={onPointerDown}
            onPointerUp={onPointerUp}
            onClick={onStageClick}
            onMouseMove={onMouseMove}
          >
            <Show when={srcAt(idx()) && !imgLoadError()} fallback={
              <div class="lightbox-broken">Image unavailable</div>
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
                {(i) => <Show when={srcAt(i)}><img src={srcAt(i)!} alt="" /></Show>}
              </For>
            </div>
          </Show>

          <Show when={count() > 1}>
            <div class="lightbox-counter">{idx() + 1} / {count()}</div>
          </Show>
          </Dialog.Content>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
    </Show>
  );
}
