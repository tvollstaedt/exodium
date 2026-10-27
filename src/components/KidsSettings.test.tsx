import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import type { KidsStatus, Playlist } from "../api/tauri";
import { KidsSettings } from "./KidsSettings";
import { requestKidsPlaylist, setKidsStatus } from "../stores/kids";

const mockInvoke = vi.mocked(invoke);

async function flush() {
  for (let i = 0; i < 4; i++) {
    await new Promise((r) => setTimeout(r, 0));
  }
}

const PLAYLISTS: Playlist[] = [
  { id: 1, name: "Rainy days", kind: "user", description: null, game_count: 3 },
  { id: 2, name: "Bedtime", kind: "user", description: null, game_count: 2 },
];

const OFF: KidsStatus = { enabled: false, locked: false, playlist_id: null, language: "EN", retry_in: 0 };
const ON: KidsStatus = { enabled: true, locked: false, playlist_id: 1, language: "EN", retry_in: 0 };

/** Answers like the backend with kids mode in `state`. */
function backend(state: KidsStatus) {
  mockInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
    switch (cmd) {
      case "get_playlists": return PLAYLISTS;
      case "kids_status": return state;
      case "kids_update": return { ...state, playlist_id: (args as { playlistId: number }).playlistId };
      default: return null;
    }
  });
}

function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  return render(() => <KidsSettings />, host);
}

const playlistSelect = () => document.querySelector<HTMLSelectElement>("[data-testid=kids-playlist]")!;

describe("KidsSettings: a playlist chosen from its own menu", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    setKidsStatus(null);
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("is preselected while kids mode is off", async () => {
    backend(OFF);
    requestKidsPlaylist(2);
    const dispose = mount();
    await flush();

    expect(playlistSelect().value).toBe("2");
    expect(mockInvoke).not.toHaveBeenCalledWith("kids_update", expect.anything());
    dispose();
  });

  it("becomes the kids playlist while kids mode is on, like picking it in the select", async () => {
    backend(ON);
    requestKidsPlaylist(2);
    const dispose = mount();
    await flush();

    expect(mockInvoke).toHaveBeenCalledWith("kids_update", { playlistId: 2, language: "EN" });
    expect(playlistSelect().value).toBe("2");
    dispose();
  });

  it("is taken once: the next opening shows the stored playlist", async () => {
    backend(ON);
    requestKidsPlaylist(2);
    const first = mount();
    await flush();
    first();
    document.body.innerHTML = "";
    mockInvoke.mockClear();

    const dispose = mount();
    await flush();
    expect(mockInvoke).not.toHaveBeenCalledWith("kids_update", expect.anything());
    expect(playlistSelect().value).toBe("1");
    dispose();
  });
});
