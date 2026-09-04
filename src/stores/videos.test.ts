import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";

const mockInvoke = vi.mocked(invoke);

const PROBING = { phase: "probing", progress: 0, total_bytes: 0, path: null, error: null };
// total_bytes > 0 is how the panel knows a video was actually found.
const FETCHING = { phase: "fetching", progress: 0.2, total_bytes: 2_000_000, path: null, error: null };
const READY = { phase: "ready", progress: 1, total_bytes: 100, path: "/v.mp4", error: null };
const NONE = { phase: "none", progress: 0, total_bytes: 0, path: null, error: null };
/** The offline answer: same phase as "the archive has none", but carrying
 *  OFFLINE_TOKEN in `error` because no archive was ever opened (media.rs). */
const OFFLINE = { phase: "none", progress: 0, total_bytes: 0, path: null, error: "offline" };

/** Route each command to a handler so tests can script the backend. */
function backend(handlers: Record<string, (args: any) => any>) {
  mockInvoke.mockImplementation(async (cmd: string, args: any) => {
    const h = handlers[cmd];
    return h ? h(args ?? {}) : null;
  });
}

const calls = (cmd: string) =>
  mockInvoke.mock.calls.filter((c) => c[0] === cmd).map((c) => c[1] as any);

describe("video store", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    vi.resetModules();
    vi.useFakeTimers();
  });
  afterEach(() => vi.useRealTimers());

  /// The Linux freeze: without GStreamer's autoaudiosink, mounting a <video>
  /// wedges the WebKit process. When the backend says no, the store must never
  /// even start a fetch - and only an EXPLICIT no counts, so a missing command
  /// (every other test's backend returns null) leaves previews on.
  it("stands down entirely when the system cannot play video", async () => {
    backend({
      video_playback_supported: () => false,
      start_game_video: () => READY,
    });
    const store = await import("./videos");

    await store.requestVideo(1);

    expect(calls("start_game_video").length).toBe(0);
    expect(store.getVideoState(1)).toBeUndefined();
    expect(store.videoPlaybackUnsupported()).toBe(true);
  });

  it("does not re-request a video it already has", async () => {
    backend({ start_game_video: () => READY });
    const store = await import("./videos");

    await store.requestVideo(1);
    await store.requestVideo(1);

    expect(calls("start_game_video").length).toBe(1);
    expect(store.getVideoState(1)?.phase).toBe("ready");
  });

  it("stops asking about a game that has no video", async () => {
    backend({ start_game_video: () => NONE });
    const store = await import("./videos");

    await store.requestVideo(7);
    await vi.advanceTimersByTimeAsync(3000);

    expect(calls("get_video_status").length).toBe(0);
    expect(store.activeVideoCount()).toBe(0);
  });

  // Closing the panel must not throw away a fetch that is already running -
  // otherwise the next visit starts from scratch.
  it("keeps fetching after the panel moves on", async () => {
    backend({ start_game_video: () => FETCHING, get_video_status: () => FETCHING });
    const store = await import("./videos");

    store.setForegroundVideo(1);
    await store.requestVideo(1);
    store.releaseVideo(1);
    await vi.advanceTimersByTimeAsync(2100);

    expect(calls("cancel_game_video").length).toBe(0);
    expect(calls("get_video_status").length).toBeGreaterThan(1);
    expect(store.activeVideoCount()).toBe(1);
  });

  // Each fetch is a torrent stream with its own lookahead; letting an unbounded
  // number run would starve all of them.
  it("caps concurrent fetches and queues the rest", async () => {
    backend({ start_game_video: () => FETCHING, get_video_status: () => FETCHING });
    const store = await import("./videos");

    for (const id of [1, 2, 3, 4, 5]) {
      await store.requestVideo(id);
    }

    expect(store.activeVideoCount()).toBe(3);
    expect(store.queuedVideoCount()).toBe(2);
    expect(store.getVideoState(5)?.phase).toBe(store.PHASE_QUEUED);
  });

  // The bug this replaces: an evicted fetch was deleted outright, so the panel
  // showed nothing and the game looked like it simply had no video (game 5581).
  it("keeps an evicted fetch visible as queued rather than dropping it", async () => {
    backend({ start_game_video: () => FETCHING, get_video_status: () => FETCHING });
    const store = await import("./videos");

    for (const id of [1, 2, 3]) {
      store.setForegroundVideo(id);
      await store.requestVideo(id);
      store.releaseVideo(id);
    }
    // A fourth game, this one on screen, must not wait behind the others.
    store.setForegroundVideo(4);
    await store.requestVideo(4);

    expect(store.getVideoState(4)?.phase).toBe("fetching");
    const evicted = store.getVideoState(1);
    expect(evicted, "an evicted fetch must stay visible").toBeDefined();
    expect(evicted?.phase).toBe(store.PHASE_QUEUED);
    expect(calls("cancel_game_video").map((a) => a.id)).toEqual([1]);
  });

  it("starts a queued fetch as soon as a slot frees", async () => {
    let phase: any = FETCHING;
    backend({ start_game_video: () => FETCHING, get_video_status: () => phase });
    const store = await import("./videos");

    for (const id of [1, 2, 3, 4]) {
      await store.requestVideo(id);
    }
    expect(store.queuedVideoCount()).toBe(1);

    // The running ones finish; the waiting one should be picked up.
    phase = READY;
    await vi.advanceTimersByTimeAsync(1000);

    expect(store.queuedVideoCount()).toBe(0);
    expect(store.getVideoState(4)?.phase).not.toBe(store.PHASE_QUEUED);
  });

  // The panel shows nothing until the archive index confirms a video, so the
  // store must carry that knowledge across an eviction.
  it("remembers a confirmed video size when a fetch is pushed back to the queue", async () => {
    backend({ start_game_video: () => PROBING, get_video_status: () => FETCHING });
    const store = await import("./videos");

    for (const id of [1, 2, 3]) {
      store.setForegroundVideo(id);
      await store.requestVideo(id);
    }
    await vi.advanceTimersByTimeAsync(800); // all three confirm a video
    store.setForegroundVideo(4);
    await store.requestVideo(4);

    const evicted = store.getVideoState(1);
    expect(evicted?.phase).toBe(store.PHASE_QUEUED);
    expect(evicted?.total_bytes, "a confirmed video must not become unknown again")
      .toBe(2_000_000);
  });

  it("polls through the probing phase", async () => {
    let phase: any = PROBING;
    backend({ start_game_video: () => PROBING, get_video_status: () => phase });
    const store = await import("./videos");

    await store.requestVideo(9);
    expect(store.getVideoState(9)?.phase).toBe(store.PHASE_PROBING);
    await vi.advanceTimersByTimeAsync(800);
    expect(store.activeVideoCount()).toBe(1); // still working, not abandoned

    phase = NONE;
    await vi.advanceTimersByTimeAsync(800);
    expect(store.getVideoState(9)?.phase).toBe("none");
    expect(store.activeVideoCount()).toBe(0);
  });

  /** Offline the backend has no torrent session, so its "none" is the missing
   *  manager talking, not the archive. Cached, it would blacklist the game for
   *  the rest of the session - `requestVideo` refuses to ask about a game it
   *  already has an answer for - and the preview would still be missing after
   *  the app went back online. */
  it("does not remember an offline 'no video' answer", async () => {
    let answer: any = OFFLINE;
    backend({ start_game_video: () => answer });
    const store = await import("./videos");

    await store.requestVideo(11);
    await vi.advanceTimersByTimeAsync(10);
    // Nothing is left behind: the panel shows no preview, and no retry button.
    expect(store.getVideoState(11)).toBeUndefined();

    answer = READY;
    await store.requestVideo(11);
    await vi.advanceTimersByTimeAsync(10);
    expect(calls("start_game_video").length).toBe(2);
    expect(store.getVideoState(11)?.phase).toBe("ready");
  });

  /** ...while a "none" the archive itself answered stays remembered. */
  it("remembers a probe that found no video", async () => {
    backend({ start_game_video: () => NONE });
    const store = await import("./videos");

    await store.requestVideo(12);
    await vi.advanceTimersByTimeAsync(10);
    expect(store.getVideoState(12)?.phase).toBe("none");

    await store.requestVideo(12);
    expect(calls("start_game_video").length).toBe(1);
  });

  it("records the path once a fetch finishes", async () => {
    let phase: any = FETCHING;
    backend({ start_game_video: () => FETCHING, get_video_status: () => phase });
    const store = await import("./videos");

    await store.requestVideo(5);
    phase = READY;
    await vi.advanceTimersByTimeAsync(800);

    expect(store.getVideoState(5)?.path).toBe("/v.mp4");
    expect(store.activeVideoCount()).toBe(0);
  });
});
