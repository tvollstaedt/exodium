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
      if (cmd === "cancel_download") { return []; }
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

  it("cancels every queued game of a playlist, not every second one", async () => {
    const { enqueueDownloads, cancelQueued, queuedCount, MAX_ACTIVE } = await import("./batchDownloads");
    const { getDownloadState } = await import("./downloads");
    const ids = [611, 612, 613, 614, 615, 616, 617, 618];
    enqueueDownloads(11, ids.map((gameId) => ({ gameId, title: `Game ${gameId}` })));
    const queued = ids.slice(MAX_ACTIVE);
    expect(queuedCount(11)).toBe(queued.length);

    cancelQueued(11);
    expect(queuedCount(11)).toBe(0);
    expect(queued.map((id) => getDownloadState(id))).toEqual(queued.map(() => undefined));

    finished.add(611);
    finished.add(612);
    await vi.advanceTimersByTimeAsync(1100);
    expect(started()).toEqual(ids.slice(0, MAX_ACTIVE));
  });

  it("holds the queue while kids mode is locked and goes on when it unlocks", async () => {
    const { enqueueDownloads, queuedCount } = await import("./batchDownloads");
    const { getDownloadState } = await import("./downloads");
    const { setKidsStatus } = await import("./kids");
    const status = (locked: boolean) => ({ enabled: true, locked, playlist_id: 1, language: "EN", retry_in: 0 });
    const ids = [621, 622, 623, 624, 625, 626];
    enqueueDownloads(12, ids.map((gameId) => ({ gameId, title: `Game ${gameId}` })));
    expect(started()).toEqual(ids.slice(0, 4));

    // A running download finishes under the lock: its slot stays empty.
    setKidsStatus(status(true));
    finished.add(621);
    await vi.advanceTimersByTimeAsync(1100);
    expect(started()).toEqual(ids.slice(0, 4));
    expect(getDownloadState(625)?.status).toBe("Queued");
    expect(queuedCount(12)).toBe(2);

    setKidsStatus(status(false));
    expect(started()).toEqual([...ids.slice(0, 4), 625]);
    expect(queuedCount(12)).toBe(1);
  });

  it("frees a paused game's slot, and resuming puts it back in front", async () => {
    const { enqueueDownloads, resumeGameDownload, queuedCount } = await import("./batchDownloads");
    const { getDownloadState, pauseGameDownload } = await import("./downloads");
    const ids = [701, 702, 703, 704, 705, 706];
    enqueueDownloads(9, ids.map((gameId) => ({ gameId, title: `Game ${gameId}` })));

    await pauseGameDownload(702);
    expect(mockInvoke).toHaveBeenCalledWith("pause_download", { id: 702 });
    expect(getDownloadState(702)?.paused).toBe(true);
    expect(started()).toEqual([701, 702, 703, 704, 705]);

    resumeGameDownload(702);
    expect(getDownloadState(702)?.status).toBe("Queued");
    finished.add(701);
    await vi.advanceTimersByTimeAsync(1100);
    expect(started()).toEqual([701, 702, 703, 704, 705, 702]);
    expect(queuedCount(9)).toBe(1);
  });

  it("pause all holds the queue until resume all, and cancel all reaches what the backend holds", async () => {
    const { enqueueDownloads, pauseAllDownloads, resumeAllDownloads, cancelAllDownloads, queueHalted } =
      await import("./batchDownloads");
    const { getDownloadState } = await import("./downloads");
    const ids = [801, 802, 803, 804, 805];
    enqueueDownloads(10, ids.map((gameId) => ({ gameId, title: `Game ${gameId}` })));

    await pauseAllDownloads();
    expect(queueHalted()).toBe(true);
    expect(ids.slice(0, 4).every((id) => getDownloadState(id)?.paused)).toBe(true);
    expect(started()).toEqual(ids.slice(0, 4));

    resumeAllDownloads();
    expect(started()).toEqual([...ids.slice(0, 4), ...ids.slice(0, 4)]);
    expect(getDownloadState(805)?.status).toBe("Queued");

    // Pausing 804 lets 805 start; resumed, 804 waits behind the four running
    // - still the backend's, so cancel all must reach it.
    await (await import("./downloads")).pauseGameDownload(804);
    expect(getDownloadState(805)?.queued).toBeFalsy();
    (await import("./batchDownloads")).resumeGameDownload(804);
    expect(getDownloadState(804)?.held).toBe(true);

    mockInvoke.mockClear();
    await cancelAllDownloads();
    const cancelled = mockInvoke.mock.calls.filter((c) => c[0] === "cancel_download").map((c) => (c[1] as { id: number }).id);
    expect(cancelled.sort()).toEqual(ids);
    expect(ids.every((id) => getDownloadState(id) === undefined)).toBe(true);
  });

  it("restores a paused download after a restart without polling it", async () => {
    mockInvoke.mockImplementation(async (cmd: string) => (cmd === "list_active_downloads"
      ? [{ id: 901, title: "Keen", torrent_source: "eXoDOS", thumbnail_key: null, installed: false, complete: false, paused: true }]
      : null));
    const { resumeDownloads, getDownloadState } = await import("./downloads");
    await resumeDownloads();
    expect(getDownloadState(901)?.paused).toBe(true);
    await vi.advanceTimersByTimeAsync(3000);
    expect(mockInvoke.mock.calls.some((c) => c[0] === "get_download_progress")).toBe(false);
  });
});
