import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import { ContentPackSettings } from "./ContentPackSettings";

const mockInvoke = vi.mocked(invoke);

const PACK = {
  id: "posters", display_name: "Cover Art", description: "HD covers",
  size_bytes: 178_472_110, version: 1, supersedes: [],
  available: true, installed: false, installed_version: null,
};

/** Several invokes resolve before the group list settles. */
async function flush() {
  for (let i = 0; i < 6; i++) {
    await new Promise((r) => setTimeout(r, 0));
  }
}

function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const dispose = render(() => <ContentPackSettings />, host);
  return { host, dispose };
}

function groupTitles(host: HTMLElement): string[] {
  return [...host.querySelectorAll(".pack-collection-title")].map((e) => e.textContent ?? "");
}

describe("ContentPackSettings", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((async (cmd: string, args?: { collection?: string; key?: string }) => {
      if (cmd === "get_config") { return args?.key === "collections" ? "eXoDOS" : null; }
      if (cmd === "get_available_collections") {
        return [{ id: "eXoDOS", display_name: "eXoDOS", torrent_file: "", game_count: 1 }];
      }
      if (cmd === "list_content_packs") {
        return args?.collection === "eXoMedia" || args?.collection === "eXoDOS" ? [PACK] : [];
      }
      return null;
    }) as typeof invoke);
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  /** The Media Pack is not a collection (§19), so it is never in the
   *  `collections` config the other groups come from - its packs would have
   *  no way into Settings at all. */
  it("lists the reading room's packs in their own group, last", async () => {
    const { host, dispose } = mount();
    await flush();

    expect(groupTitles(host)).toEqual(["eXoDOS", "Reading Room"]);
    dispose();
  });

  it("shows no reading-room group when the manifest has no packs for it", async () => {
    mockInvoke.mockImplementation((async (cmd: string, args?: { collection?: string; key?: string }) => {
      if (cmd === "get_config") { return args?.key === "collections" ? "eXoDOS" : null; }
      if (cmd === "get_available_collections") {
        return [{ id: "eXoDOS", display_name: "eXoDOS", torrent_file: "", game_count: 1 }];
      }
      if (cmd === "list_content_packs") {
        return args?.collection === "eXoDOS" ? [PACK] : [];
      }
      return null;
    }) as typeof invoke);

    const { host, dispose } = mount();
    await flush();

    expect(host.textContent).not.toContain("Reading Room");
    dispose();
  });
});
