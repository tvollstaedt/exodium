import { createSignal, createEffect, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { Dialog } from "@ark-ui/solid/dialog";
import { convertFileSrc } from "@tauri-apps/api/core";
import { openPath } from "@tauri-apps/plugin-opener";

interface ManualViewerProps {
  path: string | null;
  kind: "pdf" | "txt" | "html" | null;
  open: boolean;
  onClose: () => void;
}

export function ManualViewer(props: ManualViewerProps) {
  const [txt, setTxt] = createSignal<string | null>(null);
  const [txtErr, setTxtErr] = createSignal(false);
  const [zoom, setZoom] = createSignal(1.0);

  createEffect(() => {
    if (!props.open) { return; }
    setZoom(1.0);
    if (props.kind !== "txt" || !props.path) { return; }
    setTxt(null);
    setTxtErr(false);
    fetch(convertFileSrc(props.path))
      .then((r) => r.text())
      .then(setTxt)
      .catch(() => setTxtErr(true));
  });

  const filename = () => props.path ? props.path.split("/").pop() ?? "Manual" : "Manual";
  const iframeSrc = () => props.path ? convertFileSrc(props.path) : "";

  const zoomIn = () => setZoom((z) => Math.min(3.0, z + 0.25));
  const zoomOut = () => setZoom((z) => Math.max(0.5, z - 0.25));
  const zoomReset = () => setZoom(1.0);
  const zoomPct = () => `${Math.round(zoom() * 100)}%`;

  const handleOpenExternal = async () => {
    if (!props.path) { return; }
    try {
      await openPath(props.path);
    } catch (e) {
      console.error("openPath failed:", e, "path:", props.path);
    }
  };

  return (
    <Show when={props.open}>
    <Dialog.Root
      open={props.open}
      onOpenChange={(e) => { if (!e.open) { props.onClose(); } }}
    >
      <Portal>
        <Dialog.Backdrop class="manual-viewer-backdrop" />
        <Dialog.Positioner class="manual-viewer-positioner">
          <Dialog.Content class="manual-viewer-content">
          <div class="manual-viewer-toolbar">
            <Dialog.Title class="manual-viewer-title">{filename()}</Dialog.Title>
            <Show when={props.kind === "pdf"}>
              <div class="manual-viewer-zoom">
                <button class="manual-viewer-zoom-btn" onClick={zoomOut} title="Zoom out">−</button>
                <button class="manual-viewer-zoom-pct" onClick={zoomReset} title="Reset zoom">{zoomPct()}</button>
                <button class="manual-viewer-zoom-btn" onClick={zoomIn} title="Zoom in">+</button>
              </div>
            </Show>
            <button class="manual-viewer-btn" onClick={handleOpenExternal} title="Open in system PDF viewer">
              ↗ Open in PDF Viewer
            </button>
            <button class="manual-viewer-close" onClick={props.onClose} title="Close (Esc)">✕</button>
          </div>

          <div class="manual-viewer-body">
            <Show when={props.kind === "pdf" || props.kind === "html"}>
              <div
                class="manual-viewer-iframe-wrap"
                style={{
                  transform: `scale(${zoom()})`,
                  "transform-origin": "top center",
                  width: `${100 / zoom()}%`,
                  height: `${100 / zoom()}%`,
                }}
              >
                <iframe
                  class="manual-viewer-iframe"
                  src={iframeSrc()}
                  sandbox={props.kind === "html" ? "allow-same-origin" : undefined}
                />
              </div>
            </Show>
            <Show when={props.kind === "txt"}>
              <Show when={txt() !== null} fallback={
                <div class="manual-viewer-loading">
                  {txtErr() ? "Failed to load manual." : "Loading…"}
                </div>
              }>
                <pre class="manual-viewer-text">{txt()}</pre>
              </Show>
            </Show>
          </div>
          </Dialog.Content>
        </Dialog.Positioner>
      </Portal>
    </Dialog.Root>
    </Show>
  );
}
