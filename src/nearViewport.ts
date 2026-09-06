/** One shared IntersectionObserver for the grid: covers load ~two screens
 *  ahead (`loading="lazy"` starts too late), and the work stays bounded when
 *  a jump-bar jump mounts the whole catalogue (§13). */

/** How far outside the viewport a card still counts as "near". ~2 screens at
 *  a typical window height; covers average 24 KB, so prefetching that many is
 *  cheap compared to the pop-in it removes. */
const ROOT_MARGIN = "1200px 0px";

const callbacks = new WeakMap<Element, () => void>();
let observer: IntersectionObserver | null = null;

function getObserver(): IntersectionObserver | null {
  if (typeof IntersectionObserver === "undefined") { return null; }
  if (!observer) {
    observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) { continue; }
          const cb = callbacks.get(entry.target);
          // One-shot: once a card has loaded its cover we neither want to
          // unload it (re-fetch churn while scrolling back) nor keep it
          // observed, so drop it from the observer immediately.
          if (cb) {
            callbacks.delete(entry.target);
            observer!.unobserve(entry.target);
            cb();
          }
        }
      },
      { rootMargin: ROOT_MARGIN },
    );
  }
  return observer;
}

/** Call `onNear` once `el` comes within the preload margin of the viewport.
 *  Without IntersectionObserver support the callback fires immediately, which
 *  degrades to today's behaviour rather than never loading an image. */
export function observeNearViewport(el: Element, onNear: () => void) {
  const obs = getObserver();
  if (!obs) { onNear(); return; }
  callbacks.set(el, onNear);
  obs.observe(el);
}

export function unobserveNearViewport(el: Element) {
  callbacks.delete(el);
  observer?.unobserve(el);
}
