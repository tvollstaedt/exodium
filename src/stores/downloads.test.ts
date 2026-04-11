import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";

const mockInvoke = vi.mocked(invoke);

// The downloads store uses module-level signals that persist for the lifetime of
// the cached module. Tests avoid state bleed by using a unique gameId per test
// (999, 100, 101, …). Do NOT reuse an ID across tests in this file — a second
// call to startGameDownload for the same ID will see leftover state from the
// first test.

function makeProgress(overrides: Partial<{
  progress: number; finished: boolean; installed: boolean; error: string | null;
}> = {}) {
  return {
    file_index: 0,
    file_name: "game.zip",
    downloaded_bytes: 50,
    total_bytes: 100,
    progress: 0.5,
    finished: false,
    installed: false,
    error: null,
    ...overrides,
  };
}

describe("downloads state machine", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    mockInvoke.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("sets initial Starting state immediately on startGameDownload", async () => {
    // Both downloadGame and getDownloadProgress resolve without error
    mockInvoke.mockResolvedValue(null);

    const { startGameDownload, getDownloadState } = await import("./downloads");
    startGameDownload(999);

    const state = getDownloadState(999);
    expect(state).toBeDefined();
    expect(state!.downloading).toBe(true);
    expect(state!.status).toBe("Starting download...");
    expect(state!.progress).toBe(0);
  });

  it("transitions to percentage status on progress poll", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "download_game") return Promise.resolve("Downloading: Doom");
      if (cmd === "get_download_progress") return Promise.resolve(makeProgress({ progress: 0.42 }));
      return Promise.resolve(null);
    });

    const { startGameDownload, getDownloadState } = await import("./downloads");
    startGameDownload(100);

    // Advance one polling interval and flush all microtasks
    await vi.advanceTimersByTimeAsync(1100);

    const state = getDownloadState(100);
    expect(state?.status).toBe("42%");
    expect(state?.progress).toBeCloseTo(0.42);
    expect(state?.downloading).toBe(true);
  });

  it("transitions to Extracting when finished but not installed", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "download_game") return Promise.resolve("ok");
      if (cmd === "get_download_progress")
        return Promise.resolve(makeProgress({ finished: true, installed: false, progress: 1 }));
      return Promise.resolve(null);
    });

    const { startGameDownload, getDownloadState } = await import("./downloads");
    startGameDownload(101);

    await vi.advanceTimersByTimeAsync(1100);

    const state = getDownloadState(101);
    expect(state?.status).toBe("Extracting...");
    expect(state?.downloading).toBe(true);
  });

  it("transitions to Installed! when installed=true and clears after 3s", async () => {
    // getGames is called by fetchGames after install — return an empty list
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "download_game") return Promise.resolve("ok");
      if (cmd === "get_download_progress")
        return Promise.resolve(makeProgress({ installed: true, finished: true, progress: 1 }));
      if (cmd === "get_games") return Promise.resolve({ games: [], total: 0 });
      return Promise.resolve(null);
    });

    const { startGameDownload, getDownloadState } = await import("./downloads");
    startGameDownload(102);

    await vi.advanceTimersByTimeAsync(1100);

    expect(getDownloadState(102)?.status).toBe("Installed!");
    expect(getDownloadState(102)?.downloading).toBe(false);

    // After 5 more seconds, the entry should be removed (delay extended to 5000ms)
    await vi.advanceTimersByTimeAsync(5100);
    expect(getDownloadState(102)).toBeUndefined();
  });

  it("sets error status when progress.error is set", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "download_game") return Promise.resolve("ok");
      if (cmd === "get_download_progress")
        return Promise.resolve(makeProgress({ error: "Download incomplete — right-click to uninstall and retry" }));
      return Promise.resolve(null);
    });

    const { startGameDownload, getDownloadState } = await import("./downloads");
    startGameDownload(103);

    await vi.advanceTimersByTimeAsync(1100);

    const state = getDownloadState(103);
    expect(state?.downloading).toBe(false);
    expect(state?.status).toContain("Download incomplete");
  });

  it("sets error status when downloadGame rejects", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "download_game") return Promise.reject(new Error("not initialized"));
      return Promise.resolve(null);
    });

    const { startGameDownload, getDownloadState } = await import("./downloads");
    startGameDownload(104);

    // Let the rejected promise propagate
    await vi.runAllTimersAsync();
    await Promise.resolve(); // extra tick

    const state = getDownloadState(104);
    // Error: prefix set, downloading stopped
    expect(state?.downloading).toBe(false);
    expect(state?.status).toMatch(/Error/i);
  });
});
