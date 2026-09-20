import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import { DocumentViewer } from "./DocumentViewer";

const mockInvoke = vi.mocked(invoke);
const calls = (cmd: string) => mockInvoke.mock.calls.filter((c) => c[0] === cmd);
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("DocumentViewer", () => {
  let dispose: (() => void) | null = null;

  beforeEach(() => mockInvoke.mockReset());
  afterEach(() => {
    dispose?.();
    dispose = null;
    document.body.innerHTML = "";
  });

  // The caller's getter rebuilds its source object and depends on state that
  // changes while the document is open (a fetch tick). Re-running here means
  // the file is resolved and downloaded again and the reader loses its page.
  it("resolves the file once while the document stays the same", async () => {
    // Never resolves: the body stays on the "resolving" fallback, so nothing
    // pulls in pdf.js and the count is the whole assertion.
    mockInvoke.mockImplementation(async (cmd: string) =>
      (cmd === "media_url" ? new Promise(() => {}) : null));
    const [tick, setTick] = createSignal(0);
    // The shape the reading room had: a fresh object, read off state that
    // moves while the document is open.
    const source = () => { tick(); return { kind: "pdf" as const, path: "/cache/pcgamer.pdf" }; };
    const host = document.createElement("div");
    document.body.appendChild(host);
    dispose = render(
      () => (
        <DocumentViewer
          open={true}
          title="PC Gamer 1995-02"
          source={source()}
          onClose={() => {}}
        />
      ),
      host,
    );
    await flush();
    expect(calls("media_url").length).toBe(1);

    setTick(1);
    await flush();
    expect(calls("media_url").length).toBe(1);
  });
});
