import { createSignal, createEffect, onCleanup, onMount, For, Show } from "solid-js";
import * as pdfjs from "pdfjs-dist";
import workerSrc from "pdfjs-dist/build/pdf.worker.mjs?url";

/** Page rendering for the Lesesaal and for game manuals. The webview's own
 *  PDF viewer is not an option: WebKitGTK has none at all, so a Linux user
 *  saw a blank iframe (#30). Rendering to canvas with pdf.js is the same
 *  picture on all three platforms. */

pdfjs.GlobalWorkerOptions.workerSrc = workerSrc;

/** pdf.js fetches these three payloads by name at runtime, so they are staged
 *  verbatim (scripts/copy-pdfjs-wasm.mjs) instead of going through the hashed
 *  asset pipeline: the JPEG 2000 / JBIG2 decoders, without which such a scan
 *  is blank pages; the CMaps a predefined-encoding PDF needs; and the base-14
 *  stand-ins for unembedded fonts. The latter two fail as boxed glyphs, with
 *  nothing but a worker warning to say why. */
const RUNTIME_URL = `${import.meta.env.BASE_URL}pdfjs/`;
const RUNTIME_PAYLOADS = {
  wasmUrl: RUNTIME_URL,
  cMapUrl: `${RUNTIME_URL}cmaps/`,
  cMapPacked: true,
  standardFontDataUrl: `${RUNTIME_URL}standard_fonts/`,
};

/** What `getDocument` gets besides the URL. On WebKitGTK the worker must not
 *  hand images over as OffscreenCanvas bitmaps: drawn into the page canvas,
 *  they intermittently arrive empty and the page comes out white with no
 *  warning anywhere - scans of every kind, a third to all of an issue (§19).
 *  Raw pixel data is correct there and faster; other engines keep the default. */
export function documentOptions(userAgent: string) {
  return /Linux/.test(userAgent)
    ? { ...RUNTIME_PAYLOADS, isOffscreenCanvasSupported: false }
    : RUNTIME_PAYLOADS;
}

/** How far ahead of the viewport a page starts rendering. A 600 dpi scan is
 *  ~1400 CSS px tall, so the old 600 px was less than half a page of lead and
 *  scrolling always arrived before the picture did. */
const RENDER_MARGIN_PX = 1600;
const RENDER_MARGIN = `${RENDER_MARGIN_PX}px 0px`;
/** How many rendered pages to hold at once. Beyond the ones on screen this is
 *  what makes paging back instant: a released page is decoded again from
 *  scratch, which for JPX + JBIG2 is a few hundred ms. One page is roughly
 *  6 MB of bitmap, so this is the memory budget as much as the cache size. */
const RENDER_BUDGET = 6;
const MIN_SCALE = 0.5;
const MAX_SCALE = 4;

/** Which pages to release so that at most `budget` stay rendered: least
 *  recently seen first, and never one that is currently on screen. */
export function evictable(
  rendered: Iterable<number>,
  visible: ReadonlySet<number>,
  recent: readonly number[],
  budget: number,
): number[] {
  const held = [...rendered];
  const over = held.length - budget;
  if (over <= 0) { return []; }
  const rank = (page: number) => {
    const i = recent.indexOf(page);
    return i === -1 ? Number.MAX_SAFE_INTEGER : i;
  };
  return held
    .filter((page) => !visible.has(page))
    .sort((a, b) => rank(b) - rank(a))
    .slice(0, over);
}

interface PdfReaderProps {
  /** URL the webview can fetch: asset protocol, or the localhost media server on Linux. */
  src: string;
  initialPage?: number;
  /** Fired when the page under the top of the viewport changes. */
  onPage?: (page: number) => void;
}

export function PdfReader(props: PdfReaderProps) {
  const [doc, setDoc] = createSignal<pdfjs.PDFDocumentProxy | null>(null);
  const [pageCount, setPageCount] = createSignal(0);
  const [current, setCurrent] = createSignal(1);
  /** Zoom as the user sees it: 1.0 fills the column. A scan carries its
   *  scanner's geometry (2533 pt wide is normal), so a zoom defined against
   *  the document's own units means 1.2 renders three times the viewport. */
  const [zoom, setZoom] = createSignal(1);
  const [fitScale, setFitScale] = createSignal(1);
  const scale = () => zoom() * fitScale();
  const [error, setError] = createSignal<string | null>(null);
  const [aspect, setAspect] = createSignal(1.4);
  /** The document's own page width in CSS pixels at scale 1. A scan carries
   *  its scanner's resolution, so this runs from ~600 to well over 1200 - a
   *  fixed layout width cropped every page that was wider than the guess. */
  const [baseWidth, setBaseWidth] = createSignal(700);
  /** The scale at which one page spans the column, less its padding. */
  const measureFit = () => {
    const width = container?.clientWidth;
    if (width) { setFitScale(Math.max((width - 48) / baseWidth(), 0.05)); }
  };
  const [query, setQuery] = createSignal("");
  const [hits, setHits] = createSignal<number[]>([]);
  const [searching, setSearching] = createSignal(false);
  /** Pages whose render threw. The observer does not fire again for a page
   *  that stays in view, so without this one failure is permanent. */
  const [failed, setFailed] = createSignal<number[]>([]);
  const hasFailed = (page: number) => failed().includes(page);

  let reader: HTMLDivElement | undefined;
  let container: HTMLDivElement | undefined;
  const canvases = new Map<number, HTMLCanvasElement>();
  const rendered = new Set<number>();
  /** Pages the observer currently reports as within the render margin. */
  const visible = new Set<number>();
  /** Page numbers, most recently seen first: what the budget evicts by. */
  const recent: number[] = [];
  let observer: IntersectionObserver | undefined;
  /** The in-flight render per page. pdf.js refuses a second render() on a
   *  canvas still busy, so the old one is cancelled and awaited first. */
  const renders = new Map<number, pdfjs.RenderTask>();
  /** Bumped on every document/scale change; a render that finishes late
   *  compares against it and throws its bitmap away. */
  let generation = 0;
  /** Bumped per search run and on unmount, so two scans cannot interleave
   *  their hits. */
  let searchGeneration = 0;

  createEffect(() => {
    const src = props.src;
    if (!src) { return; }
    let cancelled = false;
    setError(null);
    setDoc(null);
    // A render still in flight belongs to the previous document; the bump
    // makes it throw its bitmap away instead of painting it here.
    generation += 1;
    rendered.clear();
    visible.clear();
    recent.length = 0;
    const task = pdfjs.getDocument({ url: src, ...documentOptions(navigator.userAgent) });
    task.promise
      .then(async (loaded) => {
        if (cancelled) {
          void loaded.cleanup();
          return;
        }
        const first = await loaded.getPage(1);
        const viewport = first.getViewport({ scale: 1 });
        setAspect(viewport.height / viewport.width);
        setBaseWidth(viewport.width);
        measureFit();
        setPageCount(loaded.numPages);
        setDoc(loaded);
        const start = Math.min(Math.max(props.initialPage ?? 1, 1), loaded.numPages);
        setCurrent(start);
        // The placeholders exist only after the next paint. Focus goes with
        // it so PageDown works before the first click.
        requestAnimationFrame(() => {
          scrollToPage(start, "auto");
          reader?.focus();
        });
      })
      .catch((e) => {
        if (!cancelled) { setError(String(e)); }
      });
    onCleanup(() => {
      cancelled = true;
      void task.destroy();
    });
  });

  // The fit scale is a function of the column's width, so it has to follow it.
  onMount(() => {
    const onResize = () => measureFit();
    window.addEventListener("resize", onResize);
    onCleanup(() => window.removeEventListener("resize", onResize));
  });

  onCleanup(() => {
    observer?.disconnect();
    searchGeneration += 1;
    for (const page of [...renders.keys()]) { void cancelRender(page); }
    void doc()?.cleanup();
  });

  /** Cancelling rejects the task's promise; awaiting it is what guarantees
   *  the canvas is free before the next render touches it. The entry is
   *  dropped only afterwards, so a second caller waits on the same task
   *  instead of walking past a cancellation still in flight. */
  async function cancelRender(pageNumber: number) {
    const task = renders.get(pageNumber);
    if (!task) { return; }
    task.cancel();
    await task.promise.catch(() => {});
    if (renders.get(pageNumber) === task) { renders.delete(pageNumber); }
  }

  async function renderPage(pageNumber: number) {
    const document = doc();
    const canvas = canvases.get(pageNumber);
    if (!document || !canvas || rendered.has(pageNumber)) { return; }
    const mine = generation;
    rendered.add(pageNumber);
    setFailed((pages) => (pages.includes(pageNumber) ? pages.filter((p) => p !== pageNumber) : pages));
    let task: pdfjs.RenderTask | undefined;
    try {
      await cancelRender(pageNumber);
      const page = await document.getPage(pageNumber);
      if (mine !== generation) {
        rendered.delete(pageNumber);
        return;
      }
      // Zoom is a multiple of THIS page's own fit width: a two-page spread
      // in a magazine is twice as wide as the cover the placeholder was sized
      // from, and a scale shared across pages let it run out of the column.
      const natural = page.getViewport({ scale: 1 });
      const column = (container?.clientWidth ?? 0) - 48;
      const pageFit = column > 0 ? column / natural.width : fitScale();
      // Render at device resolution but lay out in CSS pixels, or the page is
      // a blur on a Retina display.
      const ratio = Math.min(window.devicePixelRatio || 1, 2);
      const viewport = page.getViewport({ scale: zoom() * pageFit * ratio });
      const context = canvas.getContext("2d");
      if (!context) { return; }
      canvas.width = viewport.width;
      canvas.height = viewport.height;
      canvas.style.width = `${viewport.width / ratio}px`;
      canvas.style.height = `${viewport.height / ratio}px`;
      const box = canvas.parentElement;
      if (box) {
        box.style.width = `${Math.round(viewport.width / ratio)}px`;
        box.style.aspectRatio = `${viewport.width} / ${viewport.height}`;
      }
      task = page.render({ canvas, canvasContext: context, viewport });
      renders.set(pageNumber, task);
      await task.promise;
      if (renders.get(pageNumber) === task) { renders.delete(pageNumber); }
      if (mine !== generation) { rendered.delete(pageNumber); }
    } catch (e) {
      if (task && renders.get(pageNumber) === task) { renders.delete(pageNumber); }
      rendered.delete(pageNumber);
      // A cancelled render is this component's own doing, not a failure.
      if ((e as { name?: string })?.name === "RenderingCancelledException") { return; }
      console.error("page render failed", pageNumber, e);
      setFailed((pages) => (pages.includes(pageNumber) ? pages : [...pages, pageNumber]));
    }
  }

  function releasePage(pageNumber: number) {
    const canvas = canvases.get(pageNumber);
    if (!canvas || !rendered.has(pageNumber)) { return; }
    rendered.delete(pageNumber);
    void cancelRender(pageNumber);
    canvas.width = 0;
    canvas.height = 0;
  }

  /** Most recently seen first. */
  function touch(pageNumber: number) {
    const at = recent.indexOf(pageNumber);
    if (at !== -1) { recent.splice(at, 1); }
    recent.unshift(pageNumber);
  }

  /** A page leaving the viewport is kept until the budget needs its memory,
   *  so a short scroll back finds it already drawn. */
  function trimRendered() {
    for (const page of evictable(rendered, visible, recent, RENDER_BUDGET)) {
      releasePage(page);
    }
  }

  function retryPage(pageNumber: number) {
    rendered.delete(pageNumber);
    void renderPage(pageNumber);
  }

  function getObserver(): IntersectionObserver {
    if (!observer) {
      observer = new IntersectionObserver(
        (entries) => {
          for (const entry of entries) {
            const page = Number((entry.target as HTMLElement).dataset.page);
            if (!page) { continue; }
            if (entry.isIntersecting) {
              visible.add(page);
              touch(page);
              void renderPage(page);
            } else {
              visible.delete(page);
              trimRendered();
            }
          }
        },
        { root: container, rootMargin: RENDER_MARGIN },
      );
    }
    return observer;
  }

  /** Re-render everything on a zoom change: the canvas bitmaps are the wrong
   *  size now, and dropping them all is simpler than rescaling. */
  createEffect(() => {
    scale();
    generation += 1;
    for (const page of [...rendered]) { releasePage(page); }
    const root = container;
    if (!root) { return; }
    requestAnimationFrame(() => {
      for (const [page, canvas] of canvases) {
        const box = canvas.parentElement?.getBoundingClientRect();
        const view = root.getBoundingClientRect();
        if (box && box.bottom > view.top - RENDER_MARGIN_PX && box.top < view.bottom + RENDER_MARGIN_PX) {
          void renderPage(page);
        }
      }
    });
  });

  // The canvas registers itself (see its ref); a parent's ref callback runs
  // while the child's ref variable is still undefined.
  function attachPage(el: HTMLDivElement, pageNumber: number) {
    el.dataset.page = String(pageNumber);
    getObserver().observe(el);
    onCleanup(() => {
      observer?.unobserve(el);
      canvases.delete(pageNumber);
      rendered.delete(pageNumber);
    });
  }

  function scrollToPage(pageNumber: number, behavior: ScrollBehavior = "smooth") {
    const target = container?.querySelector(`[data-page="${pageNumber}"]`);
    target?.scrollIntoView({ behavior, block: "start" });
  }

  function onScroll() {
    const root = container;
    if (!root) { return; }
    const top = root.getBoundingClientRect().top;
    let visible = current();
    for (const [page, canvas] of canvases) {
      const box = canvas.parentElement?.getBoundingClientRect();
      if (box && box.top <= top + 80 && box.bottom > top) {
        visible = page;
        break;
      }
    }
    if (visible !== current()) {
      setCurrent(visible);
      props.onPage?.(visible);
    }
  }

  /** Text search over the whole document. pdf.js hands out text per page, so
   *  this is a scan; only the newest run may write, or two of them interleave
   *  their hits and the older one clears "searching" under the newer. */
  async function runSearch(term: string) {
    searchGeneration += 1;
    const mineSearch = searchGeneration;
    const current = () => mineSearch === searchGeneration;
    const document = doc();
    if (!document || term.trim().length < 2) {
      setHits([]);
      return;
    }
    setSearching(true);
    const needle = term.trim().toLowerCase();
    const found: number[] = [];
    const mine = generation;
    for (let page = 1; page <= document.numPages; page++) {
      if (mine !== generation || !current() || query() !== term) { break; }
      try {
        const content = await (await document.getPage(page)).getTextContent();
        const text = content.items
          .map((item) => ("str" in item ? item.str : ""))
          .join(" ")
          .toLowerCase();
        if (text.includes(needle) && current()) {
          found.push(page);
          setHits([...found]);
        }
      } catch {
        // A page whose text layer fails to parse is not worth aborting for.
      }
    }
    if (current()) { setSearching(false); }
  }

  const zoomIn = () => setZoom((z) => Math.min(MAX_SCALE, +(z + 0.2).toFixed(2)));
  const zoomOut = () => setZoom((z) => Math.max(MIN_SCALE, +(z - 0.2).toFixed(2)));
  const fitWidth = () => {
    measureFit();
    setZoom(1);
  };

  function onKeyDown(e: KeyboardEvent) {
    // Without this the browser scrolls too, and lands somewhere between pages.
    if (e.key === "PageDown" || e.key === "ArrowRight") {
      e.preventDefault();
      scrollToPage(Math.min(current() + 1, pageCount()));
    } else if (e.key === "PageUp" || e.key === "ArrowLeft") {
      e.preventDefault();
      scrollToPage(Math.max(current() - 1, 1));
    }
  }

  return (
    <div class="pdf-reader" ref={reader} onKeyDown={onKeyDown} tabIndex={-1} data-testid="pdf-reader">
      <div class="pdf-reader-toolbar">
        <div class="pdf-reader-pager">
          <button
            class="pdf-reader-btn"
            onClick={() => scrollToPage(Math.max(current() - 1, 1))}
            disabled={current() <= 1}
            title="Previous page"
          >
            ‹
          </button>
          <input
            class="pdf-reader-page-input"
            type="number"
            min={1}
            max={pageCount()}
            value={current()}
            onChange={(e) => {
              const page = Number(e.currentTarget.value);
              if (page >= 1 && page <= pageCount()) { scrollToPage(page); }
            }}
          />
          <span class="pdf-reader-page-total">/ {pageCount() || "…"}</span>
          <button
            class="pdf-reader-btn"
            onClick={() => scrollToPage(Math.min(current() + 1, pageCount()))}
            disabled={current() >= pageCount()}
            title="Next page"
          >
            ›
          </button>
        </div>

        <div class="pdf-reader-zoom">
          <button class="pdf-reader-btn" onClick={zoomOut} title="Zoom out">−</button>
          <button class="pdf-reader-btn pdf-reader-zoom-pct" onClick={fitWidth} title="Reset zoom">
            {Math.round(zoom() * 100)}%
          </button>
          <button class="pdf-reader-btn" onClick={zoomIn} title="Zoom in">+</button>
          <button class="pdf-reader-btn" onClick={fitWidth} title="Fit page width">Fit</button>
        </div>

        <div class="pdf-reader-search">
          <input
            class="pdf-reader-search-input"
            type="search"
            placeholder="Search text - Enter"
            value={query()}
            onInput={(e) => {
              setQuery(e.currentTarget.value);
              // Emptying the box takes the hit bar with it.
              if (e.currentTarget.value.trim().length < 2) { setHits([]); }
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") { void runSearch(query()); }
              e.stopPropagation();
            }}
          />
          <Show when={query().trim().length >= 2}>
            <span class="pdf-reader-hits">
              {searching() ? "searching…" : `${hits().length} page${hits().length === 1 ? "" : "s"}`}
            </span>
          </Show>
        </div>
      </div>

      <Show when={hits().length > 0}>
        <div class="pdf-reader-hitbar">
          <span class="pdf-reader-hitbar-label">Found on pages</span>
          <For each={hits()}>
            {(page) => (
              <button class="pdf-reader-hit" onClick={() => scrollToPage(page)}>
                {page}
              </button>
            )}
          </For>
        </div>
      </Show>

      <div class="pdf-reader-pages" ref={container} onScroll={onScroll}>
        <Show when={error()}>
          <div class="pdf-reader-error">Could not open this document: {error()}</div>
        </Show>
        <Show
          when={doc()}
          fallback={
            <Show when={!error()}>
              <div class="pdf-reader-loading"><span class="btn-spinner" /> Rendering…</div>
            </Show>
          }
        >
          <For each={Array.from({ length: pageCount() }, (_, i) => i + 1)}>
            {(page) => (
              <div
                class="pdf-reader-page"
                ref={(el) => attachPage(el, page)}
                style={{ "aspect-ratio": `1 / ${aspect()}`, width: `${Math.round(baseWidth() * scale())}px` }}
              >
                <canvas
                  class="pdf-reader-canvas"
                  // Registered here, not handed to attachPage: the page div's
                  // ref callback runs before this variable exists, and an
                  // undefined canvas made renderPage bail on every page.
                  ref={(el) => canvases.set(page, el)}
                />
                <Show when={hasFailed(page)}>
                  <div class="pdf-reader-page-failed">
                    <span>Couldn't render this page</span>
                    <button class="pdf-reader-btn" onClick={() => retryPage(page)}>Retry</button>
                  </div>
                </Show>
                <span class="pdf-reader-page-number">{page}</span>
              </div>
            )}
          </For>
        </Show>
      </div>
    </div>
  );
}
