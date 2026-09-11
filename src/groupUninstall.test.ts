import { describe, it, expect, vi, beforeEach } from "vitest";

const api = vi.hoisted(() => ({
  uninstallGame: vi.fn(async (_id: number) => {}),
  resetGameData: vi.fn(async (_id: number) => ""),
}));
vi.mock("./api/tauri", () => api);

const variants = vi.hoisted(() => ({ rows: [] as any[], loadVariants: vi.fn() }));
vi.mock("./stores/variants", () => ({ loadVariants: variants.loadVariants }));
vi.mock("./stores/games", () => ({
  refreshLoadedGames: vi.fn(),
  notifyGameLibraryChanged: vi.fn(),
}));
vi.mock("./stores/downloads", () => ({
  getDownloadState: () => undefined,
  cancelGameDownload: vi.fn(async () => {}),
  stopGameDownloadTracking: vi.fn(),
}));
vi.mock("./stores/toasts", () => ({ showToast: vi.fn() }));

const row = (id: number, language: string, installed: boolean) => ({
  id, language, installed, in_library: installed, title: `Game ${language}`,
  shortcode: "AlienOdy", torrent_source: language === "EN" ? "eXoDOS" : "eXoDOS_GLP",
});

describe("performGroupUninstall", () => {
  beforeEach(() => {
    api.uninstallGame.mockClear();
    variants.loadVariants.mockImplementation(async () => variants.rows);
  });

  it("removes every installed row of the card, not just the one backing it", async () => {
    const { performGroupUninstall } = await import("./util");
    variants.rows = [row(1, "EN", true), row(2, "DE", true), row(3, "ES", false)];
    await performGroupUninstall(row(1, "EN", true) as any, () => {});
    const ids = api.uninstallGame.mock.calls.map((c) => c[0]);
    expect(ids).toEqual([1, 2]);
  });

  it("falls back to the row itself when the group cannot be read", async () => {
    const { performGroupUninstall } = await import("./util");
    variants.loadVariants.mockImplementation(async () => { throw new Error("nope"); });
    await performGroupUninstall(row(1, "EN", true) as any, () => {});
    expect(api.uninstallGame.mock.calls.map((c) => c[0])).toEqual([1]);
  });
});
