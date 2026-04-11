import { describe, it, expect, vi, beforeEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import {
  getGames,
  getGame,
  getGenres,
  getGameVariants,
  getInstalledGames,
  toggleFavorite,
  downloadGame,
  getDownloadProgress,
  uninstallGame,
  launchGame,
  getConfig,
  setConfig,
  setupStart,
  initDownloadManager,
  getAvailableCollections,
  getThumbnailDir,
} from "./tauri";

const mockInvoke = vi.mocked(invoke);

beforeEach(() => {
  mockInvoke.mockReset();
  mockInvoke.mockResolvedValue(undefined);
});

describe("API invoke mapping", () => {
  it("getGames passes camelCase args", async () => {
    mockInvoke.mockResolvedValue({ games: [], total: 0 });
    await getGames(2, 50, "doom", "Action", "year_desc", "eXoDOS", false);
    expect(mockInvoke).toHaveBeenCalledWith("get_games", {
      page: 2,
      perPage: 50,
      query: "doom",
      genre: "Action",
      sortBy: "year_desc",
      collection: "eXoDOS",
      favoritesOnly: false,
    });
  });

  it("getGame passes id", async () => {
    mockInvoke.mockResolvedValue(null);
    await getGame(42);
    expect(mockInvoke).toHaveBeenCalledWith("get_game", { id: 42 });
  });

  it("getGenres sends no args", async () => {
    mockInvoke.mockResolvedValue([]);
    await getGenres();
    expect(mockInvoke).toHaveBeenCalledWith("get_genres", { collection: undefined });
  });

  it("getGameVariants passes shortcode", async () => {
    mockInvoke.mockResolvedValue([]);
    await getGameVariants("SQ5");
    expect(mockInvoke).toHaveBeenCalledWith("get_game_variants", { shortcode: "SQ5" });
  });

  it("getInstalledGames sends no args", async () => {
    mockInvoke.mockResolvedValue([]);
    await getInstalledGames();
    expect(mockInvoke).toHaveBeenCalledWith("get_installed_games");
  });

  it("toggleFavorite passes id", async () => {
    mockInvoke.mockResolvedValue(true);
    const result = await toggleFavorite(7);
    expect(mockInvoke).toHaveBeenCalledWith("toggle_favorite", { id: 7 });
    expect(result).toBe(true);
  });

  it("downloadGame passes id", async () => {
    mockInvoke.mockResolvedValue("Downloading: Doom");
    await downloadGame(1);
    expect(mockInvoke).toHaveBeenCalledWith("download_game", { id: 1 });
  });

  it("getDownloadProgress passes id", async () => {
    mockInvoke.mockResolvedValue(null);
    await getDownloadProgress(1);
    expect(mockInvoke).toHaveBeenCalledWith("get_download_progress", { id: 1 });
  });

  it("uninstallGame passes id", async () => {
    mockInvoke.mockResolvedValue("Uninstalled: Doom");
    await uninstallGame(1);
    expect(mockInvoke).toHaveBeenCalledWith("uninstall_game", { id: 1 });
  });

  it("launchGame passes id", async () => {
    mockInvoke.mockResolvedValue("Launched: Doom");
    await launchGame(1);
    expect(mockInvoke).toHaveBeenCalledWith("launch_game", { id: 1 });
  });

  it("getConfig passes key", async () => {
    mockInvoke.mockResolvedValue("/home/user/eXoDOS");
    const val = await getConfig("data_dir");
    expect(mockInvoke).toHaveBeenCalledWith("get_config", { key: "data_dir" });
    expect(val).toBe("/home/user/eXoDOS");
  });

  it("setConfig passes key and value", async () => {
    await setConfig("data_dir", "/mnt/games");
    expect(mockInvoke).toHaveBeenCalledWith("set_config", { key: "data_dir", value: "/mnt/games" });
  });

  it("setupStart passes dataDir", async () => {
    mockInvoke.mockResolvedValue("starting");
    await setupStart("/mnt/games");
    expect(mockInvoke).toHaveBeenCalledWith("setup_start", { dataDir: "/mnt/games" });
  });

  it("initDownloadManager sends no args", async () => {
    mockInvoke.mockResolvedValue(true);
    await initDownloadManager();
    expect(mockInvoke).toHaveBeenCalledWith("init_download_manager");
  });

  it("getAvailableCollections sends no args", async () => {
    mockInvoke.mockResolvedValue([]);
    await getAvailableCollections();
    expect(mockInvoke).toHaveBeenCalledWith("get_available_collections");
  });

  it("getThumbnailDir passes collection", async () => {
    mockInvoke.mockResolvedValue("/path/to/thumbs");
    await getThumbnailDir("eXoDOS");
    expect(mockInvoke).toHaveBeenCalledWith("get_thumbnail_dir", { collection: "eXoDOS" });
  });
});
