/** Rubber-band selection inside a scroll container (§25). A drag that starts
 *  on free space within a `[data-select-zone]` reports every `[data-game-id]`
 *  of that zone the rectangle touches; a press without a drag is an empty
 *  click. Card rects are measured once at the start, in content coordinates,
 *  so scrolling during the drag costs no layout. */

export interface MarqueeHandlers {
  /** `additive`: Cmd/Ctrl held - keep what was selected before. */
  onStart: (zone: string, additive: boolean) => void;
  onSelect: (zone: string, ids: number[]) => void;
  onEmptyClick: (zone: string) => void;
}

interface Box { left: number; top: number; right: number; bottom: number }

const DRAG_THRESHOLD = 5;
const EDGE = 40;
const EDGE_SPEED = 18;
const INTERACTIVE = "[data-game-id], button, a, input, select, textarea, label, [role=button]";

export function intersecting(cards: { id: number; box: Box }[], r: Box): number[] {
  return cards
    .filter(({ box }) => box.left < r.right && box.right > r.left && box.top < r.bottom && box.bottom > r.top)
    .map(({ id }) => id);
}

export function attachMarquee(container: HTMLElement, handlers: MarqueeHandlers): () => void {
  let zone: HTMLElement | null = null;
  let zoneId = "";
  let pointerId = -1;
  let start = { x: 0, y: 0 };
  let startClient = { x: 0, y: 0 };
  let additive = false;
  let dragging = false;
  let cards: { id: number; box: Box }[] = [];
  let overlay: HTMLDivElement | null = null;
  let frame = 0;

  const origin = () => container.getBoundingClientRect();
  const toContent = (clientX: number, clientY: number) => {
    const o = origin();
    return { x: clientX - o.left + container.scrollLeft, y: clientY - o.top + container.scrollTop };
  };

  const measure = () => {
    const o = origin();
    cards = [...zone!.querySelectorAll<HTMLElement>("[data-game-id]")].map((el) => {
      const r = el.getBoundingClientRect();
      const top = r.top - o.top + container.scrollTop;
      const left = r.left - o.left + container.scrollLeft;
      return { id: Number(el.dataset.gameId), box: { left, top, right: left + r.width, bottom: top + r.height } };
    });
  };

  const update = (clientX: number, clientY: number) => {
    const current = toContent(clientX, clientY);
    const r: Box = {
      left: Math.min(start.x, current.x),
      top: Math.min(start.y, current.y),
      right: Math.max(start.x, current.x),
      bottom: Math.max(start.y, current.y),
    };
    const o = origin();
    Object.assign(overlay!.style, {
      left: `${r.left + o.left - container.scrollLeft}px`,
      top: `${r.top + o.top - container.scrollTop}px`,
      width: `${r.right - r.left}px`,
      height: `${r.bottom - r.top}px`,
    });
    handlers.onSelect(zoneId, intersecting(cards, r));
  };

  let lastClient = { x: 0, y: 0 };
  const edgeScroll = () => {
    frame = 0;
    if (!dragging) { return; }
    const o = origin();
    const dy = lastClient.y < o.top + EDGE ? -EDGE_SPEED : lastClient.y > o.bottom - EDGE ? EDGE_SPEED : 0;
    if (dy !== 0) {
      container.scrollTop += dy;
      update(lastClient.x, lastClient.y);
      frame = requestAnimationFrame(edgeScroll);
    }
  };

  const onDown = (e: PointerEvent) => {
    if (e.button !== 0) { return; }
    const target = e.target as Element;
    const z = target.closest<HTMLElement>("[data-select-zone]");
    if (!z || !container.contains(z) || target.closest(INTERACTIVE)) { return; }
    zone = z;
    zoneId = z.dataset.selectZone!;
    pointerId = e.pointerId;
    start = toContent(e.clientX, e.clientY);
    startClient = { x: e.clientX, y: e.clientY };
    lastClient = startClient;
    additive = e.metaKey || e.ctrlKey;
    dragging = false;
    container.setPointerCapture(e.pointerId);
  };

  const onMove = (e: PointerEvent) => {
    if (!zone || e.pointerId !== pointerId) { return; }
    lastClient = { x: e.clientX, y: e.clientY };
    if (!dragging) {
      if (Math.hypot(e.clientX - startClient.x, e.clientY - startClient.y) < DRAG_THRESHOLD) {
        return;
      }
      dragging = true;
      measure();
      overlay = document.createElement("div");
      overlay.className = "marquee-rect";
      document.body.appendChild(overlay);
      document.body.classList.add("is-marquee");
      handlers.onStart(zoneId, additive);
    }
    update(e.clientX, e.clientY);
    if (!frame) { frame = requestAnimationFrame(edgeScroll); }
  };

  const end = (e: PointerEvent) => {
    if (!zone || e.pointerId !== pointerId) { return; }
    if (!dragging) { handlers.onEmptyClick(zoneId); }
    if (frame) { cancelAnimationFrame(frame); frame = 0; }
    overlay?.remove();
    overlay = null;
    document.body.classList.remove("is-marquee");
    if (container.hasPointerCapture(e.pointerId)) { container.releasePointerCapture(e.pointerId); }
    zone = null;
    dragging = false;
    pointerId = -1;
    cards = [];
  };

  container.addEventListener("pointerdown", onDown);
  container.addEventListener("pointermove", onMove);
  container.addEventListener("pointerup", end);
  container.addEventListener("pointercancel", end);
  return () => {
    container.removeEventListener("pointerdown", onDown);
    container.removeEventListener("pointermove", onMove);
    container.removeEventListener("pointerup", end);
    container.removeEventListener("pointercancel", end);
    overlay?.remove();
    if (frame) { cancelAnimationFrame(frame); }
  };
}
