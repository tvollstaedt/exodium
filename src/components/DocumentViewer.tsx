import { createSignal, createEffect, createMemo, lazy, on, onCleanup, ErrorBoundary, Show, Suspense, type JSX } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { convertFileSrc } from "@tauri-apps/api/core";
import { mediaUrl, openDocument } from "../api/tauri";

// pdf.js and its worker are ~2.5 MB; they load when a document is opened,
// not with the app.
const PdfReader = lazy(async () => ({ default: (await import("./PdfReader")).PdfReader }));

export type DocumentKind = "pdf" | "image" | "txt" | "html";

export interface DocumentSource {
  kind: DocumentKind;
  /** On disk, under the data dir - what the system viewer opens. */
  path: string;
}

export interface DocumentViewerProps {
  open: boolean;
  title: string;
  subtitle?: JSX.Element;
  /** null while the caller has nothing to show yet; `placeholder` renders instead. */
  source: DocumentSource | null;
  placeholder?: JSX.Element;
  initialPage?: number;
  onPage?: (page: number) => void;
  /** Caller-specific header actions, rendered before "Open externally". */
  actions?: JSX.Element;
  onClose: () => void;
  /** Root `data-testid`; the close and external-open buttons derive theirs from it. */
  testId?: string;
}

/** The dialog shell shared by game manuals and Lesesaal issues: header,
 *  "Open externally", and the body for each document kind. */
export function DocumentViewer(props: DocumentViewerProps) {
  const [url, setUrl] = createSignal<string | null>(null);
  const [txt, setTxt] = createSignal<string | null>(null);
  const [txtErr, setTxtErr] = createSignal(false);
  const [zoom, setZoom] = createSignal(1.0);
  const [externalError, setExternalError] = createSignal<string | null>(null);

  /** The document, identified by kind and path. Both effects below start by
   *  dropping what they hold, so a caller whose getter returns a fresh object
   *  per read would re-resolve the file and reset the page. */
  const active = createMemo(
    () => (props.open ? props.source : null),
    undefined,
    { equals: (a, b) => a?.kind === b?.kind && a?.path === b?.path },
  );

  // A PDF goes through pdf.js, which fetches bytes: on Linux from the
  // localhost media server, elsewhere via the asset protocol. Everything else
  // is served by the asset protocol on every platform - an image is not
  // media, and the server's origin is not in the CSP's `img-src`.
  createEffect(on(active, (source) => {
    setUrl(null);
    setZoom(1.0);
    setExternalError(null);
    if (!source) { return; }
    if (source.kind !== "pdf") { setUrl(convertFileSrc(source.path)); return; }
    let cancelled = false;
    void mediaUrl(source.path)
      .then((u) => { if (!cancelled) { setUrl(u ?? convertFileSrc(source.path)); } })
      .catch(() => { if (!cancelled) { setUrl(convertFileSrc(source.path)); } });
    onCleanup(() => { cancelled = true; });
  }));

  createEffect(on(active, (source) => {
    setTxt(null);
    setTxtErr(false);
    if (source?.kind !== "txt") { return; }
    fetch(convertFileSrc(source.path))
      .then((r) => r.text())
      .then(setTxt)
      .catch(() => setTxtErr(true));
  }));

  const kind = () => props.source?.kind;
  const testId = (suffix: string) => (props.testId ? `${props.testId}-${suffix}` : undefined);

  const zoomIn = () => setZoom((z) => Math.min(3.0, z + 0.25));
  const zoomOut = () => setZoom((z) => Math.max(0.5, z - 0.25));
  const zoomPct = () => `${Math.round(zoom() * 100)}%`;

  const openExternal = async () => {
    const path = props.source?.path;
    if (!path) { return; }
    setExternalError(null);
    try {
      await openDocument(path);
    } catch (e) {
      console.error("openDocument failed:", e, "path:", path);
      setExternalError(String(e));
    }
  };

  const loading = (text: string) => (
    <div class="document-viewer-status">
      <span class="document-viewer-spinner"><span class="btn-spinner" /> {text}</span>
    </div>
  );

  return (
    <Show when={props.open}>
      <Dialog.Root open={true} onOpenChange={(e) => { if (!e.open) { props.onClose(); } }}>
        <Portal>
          <Dialog.Backdrop class="document-viewer-backdrop" />
          <Dialog.Positioner class="document-viewer-positioner">
            <Dialog.Content class="document-viewer-content" data-testid={props.testId}>
              <div class="document-viewer-header">
                <div class="document-viewer-titles">
                  <Dialog.Title class="document-viewer-title">{props.title}</Dialog.Title>
                  <Show when={props.subtitle}>
                    <span class="document-viewer-subtitle">{props.subtitle}</span>
                  </Show>
                </div>
                {props.actions}
                <Show when={kind() === "html"}>
                  <div class="document-viewer-zoom">
                    <button class="document-viewer-zoom-btn" onClick={zoomOut} title="Zoom out">−</button>
                    <button class="document-viewer-zoom-btn document-viewer-zoom-pct" onClick={() => setZoom(1.0)} title="Reset zoom">{zoomPct()}</button>
                    <button class="document-viewer-zoom-btn" onClick={zoomIn} title="Zoom in">+</button>
                  </div>
                </Show>
                <Show when={props.source}>
                  <button
                    class="document-viewer-btn"
                    data-testid={testId("open-external")}
                    onClick={() => void openExternal()}
                    title="Open in the system viewer"
                  >
                    ↗ Open externally
                  </button>
                </Show>
                <button class="document-viewer-close" data-testid={testId("close")} onClick={props.onClose} title="Close (Esc)">✕</button>
              </div>
              <Show when={externalError()}>
                {(msg) => (
                  <div class="document-viewer-notice" role="alert" data-testid={testId("open-external-error")}>
                    <span>Could not open externally: {msg()}</span>
                    <span class="document-viewer-notice-path">The file is at {props.source?.path}</span>
                  </div>
                )}
              </Show>

              <Show when={props.source} fallback={props.placeholder ?? loading("Opening…")}>
                <Show when={url()} fallback={loading("Resolving the file…")}>
                  {(src) => (
                    <>
                      <Show when={kind() === "pdf"}>
                        {/* The viewer is a lazy chunk; without a boundary a
                            failed import leaves the spinner up forever. */}
                        <ErrorBoundary
                          fallback={(err, reset) => (
                            <div class="document-viewer-status">
                              <p>The viewer could not be loaded: {String(err)}</p>
                              <button class="btn-secondary" onClick={reset}>Try again</button>
                            </div>
                          )}
                        >
                          <Suspense fallback={loading("Loading the viewer…")}>
                            <PdfReader src={src()} initialPage={props.initialPage} onPage={props.onPage} />
                          </Suspense>
                        </ErrorBoundary>
                      </Show>
                      <Show when={kind() === "image"}>
                        <div class="document-viewer-image">
                          <img src={src()} alt={props.title} />
                        </div>
                      </Show>
                      <Show when={kind() === "html"}>
                        <div class="document-viewer-page">
                          <div
                            class="document-viewer-iframe-wrap"
                            style={{
                              transform: `scale(${zoom()})`,
                              "transform-origin": "top center",
                              width: `${100 / zoom()}%`,
                              height: `${100 / zoom()}%`,
                            }}
                          >
                            <iframe class="document-viewer-iframe" src={src()} sandbox="allow-same-origin" />
                          </div>
                        </div>
                      </Show>
                      <Show when={kind() === "txt"}>
                        <Show when={txt() !== null} fallback={
                          <div class="document-viewer-status">
                            {txtErr() ? "Failed to load this document." : "Loading…"}
                          </div>
                        }>
                          <pre class="document-viewer-text">{txt()}</pre>
                        </Show>
                      </Show>
                    </>
                  )}
                </Show>
              </Show>
            </Dialog.Content>
          </Dialog.Positioner>
        </Portal>
      </Dialog.Root>
    </Show>
  );
}
