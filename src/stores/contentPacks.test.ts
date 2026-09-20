import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";

const mockInvoke = vi.mocked(invoke);

const PACK = {
  id: "posters", display_name: "Cover Art", description: "HD covers",
  size_bytes: 178_472_110, version: 1, supersedes: [],
  available: true, installed: true, installed_version: 1,
};

describe("content pack store", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    vi.resetModules();
  });

  /** The reading room's covers ship as a content pack, but its source is not
   *  a collection (§19) - swept only from `getAvailableCollections`, the
   *  banner and Settings would never see it. */
  it("sweeps the media source alongside the collections", async () => {
    const asked: string[] = [];
    mockInvoke.mockImplementation((async (cmd: string, args?: { collection?: string }) => {
      if (cmd === "get_available_collections") {
        return [{ id: "eXoDOS", display_name: "eXoDOS", torrent_file: "", game_count: 1 }];
      }
      if (cmd === "list_content_packs") {
        asked.push(args!.collection!);
        return [PACK];
      }
      return null;
    }) as typeof invoke);

    const { refreshInstalledPacks, packsByCollection, isPackInstalled } =
      await import("./contentPacks");
    await refreshInstalledPacks();

    expect(asked).toEqual(["eXoDOS", "eXoMedia"]);
    expect(packsByCollection()["eXoMedia"]).toHaveLength(1);
    expect(isPackInstalled("eXoMedia", "posters")).toBe(true);
  });

  it("keeps the collections when the media source has no packs", async () => {
    mockInvoke.mockImplementation((async (cmd: string, args?: { collection?: string }) => {
      if (cmd === "get_available_collections") {
        return [{ id: "eXoDOS", display_name: "eXoDOS", torrent_file: "", game_count: 1 }];
      }
      if (cmd === "list_content_packs") {
        if (args!.collection === "eXoMedia") { throw new Error("Unknown collection 'eXoMedia'"); }
        return [PACK];
      }
      return null;
    }) as typeof invoke);

    const { refreshInstalledPacks, packsByCollection } = await import("./contentPacks");
    await refreshInstalledPacks();

    expect(packsByCollection()["eXoDOS"]).toHaveLength(1);
    expect(packsByCollection()["eXoMedia"]).toBeUndefined();
  });
});
