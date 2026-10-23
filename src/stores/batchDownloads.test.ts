import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";

const mockInvoke = vi.mocked(invoke);

describe("download-all queue", () => {
  const finished = new Set<number>();

  // Fresh stores per test: a poll loop parked on the previous test's fake
  // timers never wakes, and its slot would stay taken.
  beforeEach(() => {
    vi.resetModules();
    finished.clear();
    vi.useFakeTimers();
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      const id = (args as { id?: number } | undefined)?.id ?? -1;
      if (cmd === "download_game") { return "Downloading"; }
      if (cmd === "get_download_progress") {
        return finished.has(id)
          ? { progress: 1, finished: true, installed: true, extras_done: true, error: null }
          : { progress: 0.5, finished: false, installed: false, error: null, torrent_progress: 0.5 };
      }
      return null;
    });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  const started = () => mockInvoke.mock.calls.filter((c) => c[0] === "download_game").map((c) => (c[1] as { id: number }).id);

  it("runs at most four at a time and starts the next as one finishes", async () => {
    const { enqueueDownloads, queuedCount, MAX_ACTIVE } = await import("./batchDownloads");
    const { getDownloadState } = await import("./downloads");
    const ids = [501, 502, 503, 504, 505, 506];
    enqueueDownloads(7, ids.map((gameId) => ({ gameId, title: `Game ${gameId}` })));

    expect(started()).toEqual(ids.slice(0, MAX_ACTIVE));
    expect(getDownloadState(505)?.status).toBe("Queued");
    expect(queuedCount(7)).toBe(2);

    finished.add(502);
    await vi.advanceTimersByTimeAsync(1100);
    expect(started()).toEqual([...ids.slice(0, MAX_ACTIVE), 505]);
    expect(queuedCount(7)).toBe(1);
  });

  it("drops what is still queued, leaving running downloads alone", async () => {
    const { enqueueDownloads, cancelQueued, queuedCount } = await import("./batchDownloads");
    const { getDownloadState, cancelGameDownload } = await import("./downloads");
    const ids = [601, 602, 603, 604, 605, 606];
    enqueueDownloads(8, ids.map((gameId) => ({ gameId, title: `Game ${gameId}` })));
    expect(queuedCount(8)).toBe(2);

    // Cancel on a card takes one entry out without asking the backend.
    await cancelGameDownload(605);
    expect(mockInvoke.mock.calls.some((c) => c[0] === "cancel_download")).toBe(false);
    expect(queuedCount(8)).toBe(1);

    cancelQueued(8);
    expect(queuedCount(8)).toBe(0);
    expect(getDownloadState(606)).toBeUndefined();

    finished.add(601);
    await vi.advanceTimersByTimeAsync(1100);
    expect(started()).toEqual(ids.slice(0, 4));
    expect(getDownloadState(602)?.downloading).toBe(true);
  });
});
