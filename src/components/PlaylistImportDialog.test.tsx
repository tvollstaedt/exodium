import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import type { ImportPreview } from "../api/tauri";
import { PlaylistImportDialog } from "./PlaylistImportDialog";

const mockInvoke = vi.mocked(invoke);

const PREVIEW: ImportPreview = {
  name: "Kids",
  rows: [
    { line: 1, title: "Commander Keen 4", candidates: [{ game_id: 1, title: "Commander Keen 4: Secret of the Oracle", collection: "eXoDOS", year: 1991 }] },
    { line: 2, title: "Phantasmagoria", candidates: [
      { game_id: 2, title: "Phantasmagoria", collection: "eXoDOS", year: 1995 },
      { game_id: 3, title: "Phantasmagoria", collection: "eXoScummVM", year: 1995 },
    ] },
    { line: 3, title: "Not A Game", candidates: [] },
    { line: 4, title: "commander keen 4", candidates: [{ game_id: 1, title: "Commander Keen 4: Secret of the Oracle", collection: "eXoDOS", year: 1991 }] },
  ],
};

describe("PlaylistImportDialog", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (cmd: string) => (cmd === "create_playlist_from" ? 42 : []));
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  const createButton = () => document.querySelector<HTMLButtonElement>("[data-testid=import-create]")!;
  const chips = () => [...document.querySelectorAll<HTMLButtonElement>("[data-testid=import-ambiguous] .variant-chip")];

  it("preselects eXoDOS, offers Skip and creates the playlist once per game", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(() => <PlaylistImportDialog preview={PREVIEW} onClose={() => {}} />, host);

    expect(chips().map((c) => c.textContent)).toEqual(["DOS · 1995", "ScummVM · 1995", "Skip"]);
    expect(createButton().textContent).toContain("(2)");
    expect(document.querySelector("[data-testid=import-missing]")?.textContent).toContain("Line 3: Not A Game");

    chips()[2].click();
    expect(createButton().textContent).toContain("(1)");
    chips()[1].click();
    createButton().click();
    await Promise.resolve();

    expect(mockInvoke).toHaveBeenCalledWith("create_playlist_from", { name: "Kids", gameIds: [1, 3] });
    dispose();
  });
});
