import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import { StorageTab, resetStorageCache } from "./StorageTab";

const mockInvoke = vi.mocked(invoke);

const OVERVIEW = {
  folder: "/data/eXoDOS",
  free_bytes: 200e9,
  total_bytes: 1000e9,
  used_bytes: 96e9,
  other_bytes: 704e9,
  categories: [
    { id: "games", bytes: 40e9, items: 116 },
    { id: "archives", bytes: 25e9, items: 52 },
    { id: "extras", bytes: 3e9, items: 40 },
    { id: "saves", bytes: 0, items: 0 },
    { id: "support", bytes: 8e9, items: 0 },
    { id: "packs", bytes: 10e9, items: 0 },
    { id: "pack_archives", bytes: 5e9, items: 3 },
    { id: "reading", bytes: 2e9, items: 24 },
    { id: "caches", bytes: 1e9, items: 0 },
    { id: "configs", bytes: 0.5e9, items: 0 },
    { id: "other", bytes: 1.5e9, items: 0 },
  ],
};

const GAMES = [
  { id: 1, title: "Grim Fandango", collection: "eXoScummVM", language: "EN", game_bytes: 1.1e9, archive_bytes: 1.1e9, save_bytes: 0, last_played: null },
  { id: 2, title: "Dark Side", collection: "eXoScummVM", language: "EN", game_bytes: 463e3, archive_bytes: 0, save_bytes: 0, last_played: "2026-09-19T22:00:00Z" },
];

describe("StorageTab", () => {
  beforeEach(() => {
    resetStorageCache();
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "storage_overview") { return OVERVIEW; }
      if (cmd === "installed_games_storage") { return GAMES; }
      if (cmd === "archive_usage") { return { count: 52, bytes: 25e9 }; }
      if (cmd === "get_config") { return "0"; }
      return null;
    });
  });
  afterEach(() => { document.body.innerHTML = ""; });

  it("lists categories with sizes and the games largest first", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(() => <StorageTab active={true} onGoToPacks={() => {}} />, host);
    await new Promise((r) => setTimeout(r, 30));
    const text = host.textContent ?? "";
    expect(text).toContain("Game archives");
    expect(text).toContain("25.0 GB");
    expect(text).toContain("52 archives");
    expect(text).toContain("52 archives of installed games");
    expect(text).toContain("Not kept after install");
    expect(text).not.toContain("Save backups");
    const rows = Array.from(host.querySelectorAll("[data-testid=storage-game]")).map((r) => r.textContent ?? "");
    expect(rows[0]).toContain("Grim Fandango");
    expect(rows[0]).toContain("2.2 GB");
    expect(rows[1]).toContain("Dark Side");
    dispose();
  });

  /** A native <select> is drawn by the platform: on WebKitGTK that is a white
   *  box with a near-invisible list inside the dark dialog. The sort control
   *  is the shared Ark Select like every other dropdown. */
  it("sorts with the app's own dropdown, not a native select", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(() => <StorageTab active={true} onGoToPacks={() => {}} />, host);
    await new Promise((r) => setTimeout(r, 30));
    const sort = host.querySelector(".storage-sort");
    expect(sort?.querySelector("select")).toBeNull();
    expect(sort?.querySelector(".ark-select-trigger")?.textContent).toContain("Size");
    dispose();
  });

  /** The walk takes seconds on a large library: a measurement survives
   *  switching sections and reopening the dialog (module-level state). */
  it("keeps the measurement across mounts", async () => {
    const mount = () => {
      const host = document.createElement("div");
      document.body.appendChild(host);
      const dispose = render(() => <StorageTab active={true} onGoToPacks={() => {}} />, host);
      return { host, dispose };
    };
    const first = mount();
    await new Promise((r) => setTimeout(r, 30));
    first.dispose();
    const second = mount();
    await new Promise((r) => setTimeout(r, 30));
    expect(mockInvoke.mock.calls.filter((c) => c[0] === "storage_overview").length).toBe(1);
    expect(second.host.textContent).toContain("Grim Fandango");
    second.dispose();
  });
});
