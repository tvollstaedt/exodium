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

/** PDFs render natively inside a Tauri WebView iframe (WebKit on mac/Linux,
 *  WebView2 on Windows — all three ship a built-in PDF viewer). For .txt we
 *  fetch the file via the asset protocol and render as <pre>. For .html we
 *  sandbox an iframe. "Open externally" hands off to the OS default via the
 *  tauri-plugin-opener. */
export function ManualViewer(props: ManualViewerProps) {
  const [txt, setTxt] = createSignal<string | null>(null);
  const [txtErr, setTxtErr] = createSignal(false);

  createEffect(() => {
    if (!props.open || props.kind !== "txt" || !props.path) { return; }
    setTxt(null);
    setTxtErr(false);
    fetch(convertFileSrc(props.path))
      .then((r) => r.text())
      .then(setTxt)
      .catch(() => setTxtErr(true));
  });

  const filename = () => props.path ? props.path.split("/").pop() ?? "Manual" : "Manual";
  const iframeSrc = () => props.path ? convertFileSrc(props.path) : "";

  const handleOpenExternal = async () => {
    if (!props.path) { return; }
    try { await openPath(props.path); } catch { /* non-fatal */ }
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
            <button class="manual-viewer-btn" onClick={handleOpenExternal} title="Open in default app">
              ↗ Open externally
            </button>
            <button class="manual-viewer-close" onClick={props.onClose} title="Close (Esc)">✕</button>
          </div>

          <div class="manual-viewer-body">
            <Show when={props.kind === "pdf" || props.kind === "html"}>
              <iframe
                class="manual-viewer-iframe"
                src={iframeSrc()}
                sandbox={props.kind === "html" ? "allow-same-origin" : undefined}
              />
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
