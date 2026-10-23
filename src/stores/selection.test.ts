import { describe, it, expect, beforeEach, vi } from "vitest";
import type { Game } from "../api/tauri";

const game = (id: number, over: Partial<Game> = {}): Game =>
  ({ id, title: `Game ${id}`, language: "EN", installed: false, in_library: false, favorited: false, torrent_source: "eXoDOS", thumbnail_key: null, available_languages: null, ...over }) as Game;

describe("selection", () => {
  beforeEach(() => { vi.resetModules(); });

  it("toggles, ranges from the anchor and starts over in another list", async () => {
    const s = await import("./selection");
    const list = [1, 2, 3, 4, 5].map((id) => game(id));
    s.toggleSelected("browse", list[1]);
    s.selectRange("browse", list, list[3]);
    expect(s.selectedRefs().map((r) => r.id)).toEqual([2, 3, 4]);
    expect(s.isSelected("browse", 3)).toBe(true);
    expect(s.isSelected("shelf:installed", 3)).toBe(false);

    s.toggleSelected("browse", list[2]);
    expect(s.selectedRefs().map((r) => r.id)).toEqual([2, 4]);

    s.toggleSelected("shelf:installed", list[0]);
    expect(s.selectionScope()).toBe("shelf:installed");
    expect(s.selectedRefs().map((r) => r.id)).toEqual([1]);
  });

  it("keeps the earlier selection only for an additive frame", async () => {
    const s = await import("./selection");
    s.toggleSelected("browse", game(1));
    const two = s.refOf(game(2))!;
    s.selectRefs("browse", [...s.selectedRefs(), two]);
    expect(s.selectionCount()).toBe(2);
    s.selectRefs("browse", [two]);
    expect(s.selectedRefs().map((r) => r.id)).toEqual([2]);
  });

  it("reads a merged card as installed when another language is", async () => {
    const s = await import("./selection");
    const ref = s.refOf(game(7, { available_languages: "EN:0,DE:2" }))!;
    expect(ref.installed).toBe(true);
    expect(s.refOf(game(8, { available_languages: "EN:0,DE:1" }))!.in_library).toBe(true);
  });
});
