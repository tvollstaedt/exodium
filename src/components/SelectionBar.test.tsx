import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import type { GameRef, Playlist } from "../api/tauri";
import { SelectionBar } from "./SelectionBar";
import { clearSelection, selectRefs } from "../stores/selection";

const mockInvoke = vi.mocked(invoke);
const ref = (id: number, over: Partial<GameRef> = {}): GameRef =>
  ({ id, title: `Game ${id}`, torrent_source: "eXoDOS", thumbnail_key: null, installed: false, in_library: false, favorited: false, ...over });
const KIDS: Playlist = { id: 9, name: "Kids", kind: "user", description: null, game_count: 2 };

describe("SelectionBar", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (cmd: string) => (cmd === "get_playlists" ? [] : null));
    clearSelection();
  });
  afterEach(() => { document.body.innerHTML = ""; });

  const mount = (playlist: Playlist | null) => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    return render(() => <SelectionBar playlist={playlist} onSelectAll={() => {}} onChanged={() => {}} />, host);
  };
  const button = (label: RegExp) =>
    [...document.querySelectorAll<HTMLButtonElement>(".selection-bar button")].find((b) => label.test(b.textContent ?? ""));

  it("offers Remove only for a user playlist and Unfavorite once all are favorites", async () => {
    selectRefs("browse", [ref(1, { favorited: true }), ref(2, { favorited: true })]);
    const dispose = mount(null);
    expect(document.querySelector("[data-testid=selection-count]")?.textContent).toBe("2 games selected");
    expect(button(/Remove/)).toBeUndefined();
    expect(button(/Unfavorite/)).toBeDefined();
    button(/Unfavorite/)!.click();
    await Promise.resolve();
    expect(mockInvoke).toHaveBeenCalledWith("set_favorites", { ids: [1, 2], favorited: false });
    dispose();

    const again = mount(KIDS);
    button(/Remove from "Kids"/)!.click();
    await Promise.resolve();
    expect(mockInvoke).toHaveBeenCalledWith("set_playlist_membership_many", { playlistId: 9, gameIds: [1, 2], member: false });
    again();
  });

  it("disables Download when every selected game is installed or downloading", () => {
    selectRefs("browse", [ref(1, { installed: true }), ref(2, { in_library: true })]);
    const dispose = mount(null);
    expect(button(/^Download$/)!.disabled).toBe(true);
    dispose();
  });
});
