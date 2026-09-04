import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

/** The Browse list the store walks in list mode. */
const list = vi.hoisted(() => ({
  rows: [] as any[],
  more: false,
  fetchMore: vi.fn(async () => {}),
}));
vi.mock("./games", () => ({
  games: () => list.rows,
  hasMore: () => list.more,
  fetchMoreGames: list.fetchMore,
}));

/** The network mode, without the manager/pack machinery behind the real
 *  store. Offline is a state the player has to refuse to walk in. */
const net = vi.hoisted(() => ({ offline: false }));
vi.mock("./network", () => ({
  isOffline: () => net.offline,
  networkMode: () => (net.offline ? "offline" : "live"),
}));

const mockInvoke = vi.mocked(invoke);

const PROBING = { phase: "probing", progress: 0, total_bytes: 0, path: null, error: null };
const FETCHING = { phase: "fetching", progress: 0.5, total_bytes: 3_000_000, path: null, error: null };
const NONE = { phase: "none", progress: 0, total_bytes: 0, path: null, error: null };
const ready = (id: number) => ({ phase: "ready", progress: 1, total_bytes: 100, path: `/musiccache/eXoDOS_${id}.mp3`, error: null });

const game = (id: number) => ({ id, title: `Game ${id}`, torrent_source: "eXoDOS", thumbnail_key: `k${id}` });
/** A Browse row: a game plus the catalogue's theme hint. */
const row = (id: number, music: string | null = `Game ${id}.mp3`) => ({ ...game(id), music_file: music });
const candidate = (id: number, ext = "mp3") => ({ id, title: `Game ${id}`, torrent_source: "eXoDOS", thumbnail_key: `k${id}`, music_file: `Game ${id}.${ext}` });

function backend(handlers: Record<string, (args: any) => any>) {
  mockInvoke.mockImplementation(async (cmd: string, args: any) => {
    const h = handlers[cmd];
    return h ? h(args ?? {}) : null;
  });
}
const calls = (cmd: string) => mockInvoke.mock.calls.filter((c) => c[0] === cmd).map((c) => c[1] as any);

/** A fake <audio>: records what the store asked of it. */
function fakePort() {
  let ended: () => void = () => {};
  const port = {
    src: null as string | null,
    volume: 1,
    playCalls: 0,
    pauseCalls: 0,
    setSrc(url: string | null) { port.src = url; },
    play: async () => { port.playCalls++; },
    pause: () => { port.pauseCalls++; },
    setVolume(v: number) { port.volume = v; },
    onEnded(cb: () => void) { ended = cb; },
    end: () => ended(),
  };
  return port;
}

async function settle(ms = 0) {
  await vi.advanceTimersByTimeAsync(ms);
}

describe("music store", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    vi.resetModules();
    vi.useFakeTimers();
    list.rows = [];
    list.more = false;
    list.fetchMore.mockReset();
    list.fetchMore.mockImplementation(async () => {});
    net.offline = false;
  });
  afterEach(() => vi.useRealTimers());

  it("importing the store never asks for shuffle candidates", async () => {
    backend({});
    await import("./music");
    await settle(100);
    expect(calls("music_shuffle_candidates").length).toBe(0);
  });

  it("plays a theme once its bytes arrive, and keeps playing when the next game has none", async () => {
    let phase: any = FETCHING;
    backend({ start_game_music: ({ id }: any) => (id === 1 ? PROBING : NONE), get_music_status: () => phase });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);

    store.playTheme(game(1));
    await settle(10);
    expect(store.currentTrack()).toBeNull();
    expect(port.playCalls).toBe(0);

    phase = ready(1);
    await settle(800);
    expect(store.currentTrack()?.gameId).toBe(1);
    expect(port.src).toContain("eXoDOS_1.mp3");
    expect(port.playCalls).toBe(1);
    expect(store.musicPlaying()).toBe(true);

    // A game without a theme does not silence the one that is playing.
    store.playTheme(game(2));
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(1);
    expect(port.pauseCalls).toBe(0);
    expect(store.wantedTrack()).toBeNull();
  });

  it("pauses for an unmuted video and resumes when it ends", async () => {
    backend({ start_game_music: ({ id }: any) => ready(id) });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);
    store.playTheme(game(1));
    await settle(10);
    expect(store.musicPlaying()).toBe(true);

    store.pauseFor("video");
    expect(store.musicPlaying()).toBe(false);
    expect(port.pauseCalls).toBe(1);

    store.resumeFrom("video");
    await settle(0);
    expect(store.musicPlaying()).toBe(true);
    expect(port.playCalls).toBe(2);
  });

  it("resumes only when every reason is withdrawn", async () => {
    backend({ start_game_music: ({ id }: any) => ready(id) });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);
    store.playTheme(game(1));
    await settle(10);

    store.pauseFor("video");
    store.pauseFor("game");
    store.resumeFrom("video");
    await settle(0);
    expect(store.musicPlaying()).toBe(false);

    store.resumeFrom("game");
    await settle(0);
    expect(store.musicPlaying()).toBe(true);
  });

  // A game launched into silence must not start music when it exits.
  it("ignores a game exit that did not pause anything", async () => {
    backend({ start_game_music: ({ id }: any) => ready(id) });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);
    store.playTheme(game(1));
    await settle(10);
    store.togglePlay(); // the listener paused it
    expect(store.musicPlaying()).toBe(false);

    store.pauseFor("game"); // launch while already silent
    store.resumeFrom("game"); // exit
    await settle(0);

    expect(store.musicPlaying()).toBe(false);
    expect(port.playCalls).toBe(1);
  });

  it("a user pause outlives a withdrawn reason", async () => {
    backend({ start_game_music: ({ id }: any) => ready(id) });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);
    store.playTheme(game(1));
    await settle(10);

    store.pauseFor("video");
    store.togglePlay(); // paused already: this is "play" - the click wins
    await settle(0);
    expect(store.musicPlaying()).toBe(true);
    store.togglePlay(); // now the listener pauses
    store.pauseFor("video");
    store.resumeFrom("video");
    await settle(0);
    expect(store.musicPlaying()).toBe(false);
  });

  it("shuffle keeps exactly one track ahead and moves on when a song ends", async () => {
    backend({
      music_shuffle_candidates: () => [candidate(10), candidate(11), candidate(12), candidate(13)],
      start_game_music: ({ id }: any) => ready(id),
    });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);

    await store.startShuffle();
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(10);
    // Current plus one prefetch - not the whole batch.
    expect(calls("start_game_music").map((a) => a.id)).toEqual([10, 11]);

    port.end();
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(11);
    expect(calls("start_game_music").map((a) => a.id)).toEqual([10, 11, 12]);
  });

  it("continuous off stops at the end of a track, in both queue modes", async () => {
    list.rows = [row(1), row(2)];
    backend({
      music_shuffle_candidates: () => [candidate(10), candidate(11)],
      start_game_music: ({ id }: any) => ready(id),
    });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);
    await store.setMusicContinuous(false);

    await store.startShuffle();
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(10);
    // Nothing is prefetched either - the track would never be played.
    expect(calls("start_game_music").map((a) => a.id)).toEqual([10]);

    port.end();
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(10);
    expect(store.musicPlaying()).toBe(false);

    // The skip button is the listener's word and still moves the queue.
    await store.next();
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(11);

    store.playFromList(row(1));
    await settle(10);
    port.end();
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(1);
  });

  it("drops a shuffle pick that never arrives, without a word", async () => {
    let phase: any = FETCHING;
    backend({
      music_shuffle_candidates: () => [candidate(10), candidate(11)],
      start_game_music: ({ id }: any) => (id === 10 ? FETCHING : ready(id)),
      get_music_status: () => phase,
    });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);

    await store.startShuffle();
    await settle(10);
    expect(store.wantedTrack()?.gameId).toBe(10);

    await settle(store.SHUFFLE_SKIP_MS + 10);
    expect(calls("cancel_game_music").map((a) => a.id)).toContain(10);
    expect(store.currentTrack()?.gameId).toBe(11);
  });

  it("skips .ogg candidates when the system cannot decode them", async () => {
    backend({
      music_playback_supported: () => ({ mp3: true, ogg: false }),
      music_shuffle_candidates: () => [candidate(10, "ogg"), candidate(11)],
      start_game_music: ({ id }: any) => ready(id),
    });
    const store = await import("./music");
    store.attachAudio(fakePort());
    await store.requestTheme(1); // learns the support answer
    await store.startShuffle();
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(11);
  });

  it("list mode follows the list and prefetches exactly one row ahead", async () => {
    list.rows = [row(1), row(2, null), row(3)];
    backend({ start_game_music: ({ id }: any) => ready(id) });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);

    store.playFromList(row(1));
    await settle(10);
    expect(store.musicMode()).toBe("list");
    expect(store.currentTrack()?.gameId).toBe(1);
    // Game 2 has no theme to offer, so the one track ahead is game 3.
    expect(calls("start_game_music").map((a) => a.id)).toEqual([1, 3]);

    port.end();
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(3);
  });

  it("list mode skips a row already known to have no theme", async () => {
    list.rows = [row(1), row(2), row(3)];
    backend({ start_game_music: ({ id }: any) => (id === 2 ? NONE : ready(id)) });
    const store = await import("./music");
    store.attachAudio(fakePort());

    await store.requestTheme(2); // the probe comes back empty
    await settle(10);
    expect(store.musicNone().has(2)).toBe(true);

    store.playFromList(row(1));
    await settle(10);
    expect(calls("start_game_music").map((a) => a.id)).toEqual([2, 1, 3]);
  });

  it("a wanted list track without a theme hands over to the next row", async () => {
    list.rows = [row(1), row(2)];
    backend({ start_game_music: ({ id }: any) => (id === 1 ? NONE : ready(id)) });
    const store = await import("./music");
    store.attachAudio(fakePort());

    store.playFromList(row(1));
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(2);
  });

  it("the end of a complete list stops playback without a word", async () => {
    list.rows = [row(1)];
    backend({ start_game_music: ({ id }: any) => ready(id) });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);

    store.playFromList(row(1));
    await settle(10);
    port.end();
    await settle(10);

    expect(store.currentTrack()?.gameId).toBe(1);
    expect(store.wantedTrack()).toBeNull();
    expect(store.musicPlaying()).toBe(false);
    expect(list.fetchMore).not.toHaveBeenCalled();
  });

  it("the end of a loaded page fetches the next one exactly once", async () => {
    list.rows = [row(1)];
    list.more = true;
    list.fetchMore.mockImplementation(async () => {
      list.rows = [...list.rows, row(2)];
      list.more = false;
    });
    backend({ start_game_music: ({ id }: any) => ready(id) });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);

    store.playFromList(row(1));
    await settle(10);
    port.end();
    await settle(10);

    expect(store.currentTrack()?.gameId).toBe(2);
    expect(list.fetchMore).toHaveBeenCalledTimes(1);
  });

  it("playing a theme leaves list mode", async () => {
    list.rows = [row(1), row(2)];
    backend({ start_game_music: ({ id }: any) => ready(id) });
    const store = await import("./music");
    store.attachAudio(fakePort());

    store.playFromList(row(1));
    await settle(10);
    expect(store.musicMode()).toBe("list");

    store.playTheme(game(2));
    await settle(10);
    expect(store.musicMode()).toBe("theme");
    expect(store.currentTrack()?.gameId).toBe(2);
  });

  it("a fetch's outcome moves the id between the cached and none sets", async () => {
    backend({ start_game_music: ({ id }: any) => (id === 5 ? NONE : ready(id)) });
    const store = await import("./music");
    store.attachAudio(fakePort());

    await store.requestTheme(4);
    await store.requestTheme(5);
    await settle(10);

    expect(store.musicCached().has(4)).toBe(true);
    expect(store.musicNone().has(4)).toBe(false);
    expect(store.musicNone().has(5)).toBe(true);
    expect(store.musicCached().has(5)).toBe(false);
    expect(store.playableHint(row(4))).toBe(true);
    expect(store.playableHint(row(5))).toBe(false);
    expect(store.playableHint(row(6, null))).toBe(false);
  });

  it("refreshMusicIndex asks the backend once and fills both sets", async () => {
    backend({ music_cache_index: () => ({ cached: [1, 2], none: [3] }) });
    const store = await import("./music");

    await Promise.all([store.refreshMusicIndex(), store.refreshMusicIndex()]);

    expect(calls("music_cache_index").length).toBe(1);
    expect([...store.musicCached()]).toEqual([1, 2]);
    expect([...store.musicNone()]).toEqual([3]);
  });

  it("hiding the player pauses it and keeps the track loaded", async () => {
    backend({ start_game_music: ({ id }: any) => ready(id) });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);
    store.playTheme(game(1));
    await settle(10);
    expect(store.playerHidden()).toBe(false);

    store.hidePlayer();
    expect(store.playerHidden()).toBe(true);
    expect(store.musicPlaying()).toBe(false);
    expect(store.musicUserPaused()).toBe(true);
    expect(port.pauseCalls).toBe(1);
    // The × is not the stop button: the track stays where it was.
    expect(store.currentTrack()?.gameId).toBe(1);
    expect(port.src).toContain("eXoDOS_1.mp3");

    // Nothing may resume behind a hidden bar - a game exiting least of all.
    store.pauseFor("game");
    store.resumeFrom("game");
    await settle(0);
    expect(store.musicPlaying()).toBe(false);
    expect(port.playCalls).toBe(1);

    // Showing it again is not playing it.
    store.showPlayer();
    expect(store.playerHidden()).toBe(false);
    expect(store.musicPlaying()).toBe(false);
  });

  it("starting music brings a hidden player back", async () => {
    list.rows = [row(1), row(2)];
    backend({ start_game_music: ({ id }: any) => ready(id) });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);
    store.playTheme(game(1));
    await settle(10);

    store.hidePlayer();
    store.playTheme(game(2));
    await settle(10);
    expect(store.playerHidden()).toBe(false);
    expect(store.musicPlaying()).toBe(true);

    store.hidePlayer();
    store.playFromList(row(1));
    await settle(10);
    expect(store.playerHidden()).toBe(false);
    expect(store.musicPlaying()).toBe(true);
  });

  // Offline the backend has no torrent manager and answers every pick with an
  // instant "none" - a walk would run through the whole catalogue in one tick.
  it("offline, a queue walk stops at the first pick instead of spinning", async () => {
    net.offline = true;
    list.rows = [row(1), row(2), row(3)];
    backend({
      music_shuffle_candidates: () => [candidate(10), candidate(11)],
      start_game_music: () => NONE,
    });
    const store = await import("./music");
    store.attachAudio(fakePort());

    await store.startShuffle();
    store.playFromList(row(1));
    await settle(2000);

    // The shuffle refuses to start at all; the list walk probes its first row
    // and stops on the answer.
    expect(calls("music_shuffle_candidates").length).toBe(0);
    expect(calls("start_game_music").length).toBe(1);
  });

  it("does not remember an offline 'no music' answer", async () => {
    net.offline = true;
    backend({ start_game_music: () => NONE });
    const store = await import("./music");
    store.attachAudio(fakePort());

    await store.requestTheme(1);
    await settle(10);
    // Offline that "none" is the missing manager talking, not the archive.
    expect(store.musicNone().has(1)).toBe(false);
    expect(store.getMusicState(1)).toBeUndefined();

    net.offline = false;
    await store.requestTheme(1);
    await settle(10);
    expect(calls("start_game_music").length).toBe(2);
    expect(store.musicNone().has(1)).toBe(true);
  });

  it("a queue that finds nothing anywhere stops after a handful of duds", async () => {
    backend({
      music_shuffle_candidates: () => Array.from({ length: 10 }, (_, i) => candidate(20 + i)),
      start_game_music: () => NONE,
    });
    const store = await import("./music");
    store.attachAudio(fakePort());

    await store.startShuffle();
    await settle(2000);

    // Exactly AUTO_SKIP_MAX picks, then the walk gives up.
    expect(calls("start_game_music").length).toBe(5);
    expect(store.wantedTrack()).toBeNull();
  });

  it("a skipped fetch is forgotten, so the next request probes again", async () => {
    backend({
      music_shuffle_candidates: () => [candidate(10), candidate(11)],
      start_game_music: () => FETCHING,
      get_music_status: () => FETCHING,
    });
    const store = await import("./music");
    store.attachAudio(fakePort());

    await store.startShuffle();
    await settle(10);
    await settle(store.SHUFFLE_SKIP_MS + 10);

    expect(calls("cancel_game_music").map((a) => a.id)).toContain(10);
    expect(store.getMusicState(10)).toBeUndefined();

    await store.requestTheme(10);
    expect(calls("start_game_music").filter((a) => a.id === 10).length).toBe(2);
  });

  it("a fetch that loses its reason while queued never reaches the backend", async () => {
    let firstDone = false;
    backend({
      music_shuffle_candidates: () => [candidate(10), candidate(11)],
      start_game_music: ({ id }: any) => (id === 10 ? ready(10) : FETCHING),
      get_music_status: ({ id }: any) => (id === 1 && firstDone ? ready(1) : FETCHING),
    });
    const store = await import("./music");
    store.attachAudio(fakePort());

    // Three background probes hold every slot, so the prefetch has to wait.
    for (const id of [1, 2, 3]) { await store.requestTheme(id); }
    await store.startShuffle();
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(10);
    expect(store.getMusicState(11)?.phase).toBe(store.MUSIC_QUEUED);

    store.stop();
    firstDone = true;
    await settle(1500); // a slot frees and the queue pumps

    expect(calls("start_game_music").map((a) => a.id)).not.toContain(11);
  });

  it("a second shuffle click while a pick is loading does not stack a fetch", async () => {
    backend({
      music_shuffle_candidates: () => [candidate(10), candidate(11)],
      start_game_music: () => FETCHING,
      get_music_status: () => FETCHING,
    });
    const store = await import("./music");
    store.attachAudio(fakePort());

    await store.startShuffle();
    await settle(10);
    expect(store.wantedTrack()?.gameId).toBe(10);

    await store.startShuffle();
    await settle(10);

    expect(calls("start_game_music").length).toBe(1);
  });

  it("a launch during a fetch keeps the arriving track quiet", async () => {
    let phase: any = FETCHING;
    backend({ start_game_music: () => PROBING, get_music_status: () => phase });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);

    store.playTheme(game(1));
    await settle(10);
    expect(store.wantedTrack()?.gameId).toBe(1);

    store.pauseFor("game");
    phase = ready(1);
    await settle(800);

    expect(store.currentTrack()?.gameId).toBe(1);
    expect(store.musicPlaying()).toBe(false);
    expect(port.playCalls).toBe(0);

    store.resumeFrom("game");
    await settle(0);
    expect(store.musicPlaying()).toBe(true);
  });

  it("two running games are two reasons to stay quiet", async () => {
    backend({ start_game_music: ({ id }: any) => ready(id) });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);
    store.playTheme(game(1));
    await settle(10);

    store.pauseForGame(11);
    store.pauseForGame(12);
    expect(store.musicPlaying()).toBe(false);

    // The first game exiting must not undo the second one's claim.
    store.resumeFromGame(11);
    await settle(0);
    expect(store.musicPlaying()).toBe(false);

    store.resumeFromGame(12);
    await settle(0);
    expect(store.musicPlaying()).toBe(true);
  });

  it("initMusic loads the cache index once", async () => {
    backend({ music_cache_index: () => ({ cached: [7], none: [8] }) });
    const store = await import("./music");

    await store.initMusic();
    await settle(10);

    expect(calls("music_cache_index").length).toBe(1);
    expect(store.musicCached().has(7)).toBe(true);
  });

  it("offers no play button for a container the system cannot decode", async () => {
    backend({ music_playback_supported: () => ({ mp3: false, ogg: false }) });
    const store = await import("./music");

    await store.requestTheme(1); // learns the support answer
    expect(store.playableHint(row(1))).toBe(false);
    expect(calls("start_game_music").length).toBe(0);
  });

  it("a cached track is playable whatever the catalogue says", async () => {
    backend({ music_cache_index: () => ({ cached: [4], none: [] }) });
    const store = await import("./music");

    await store.refreshMusicIndex();

    expect(store.playableHint(row(4, null))).toBe(true);
  });

  it("stepping back twice reaches the track before last", async () => {
    backend({
      music_shuffle_candidates: () => [candidate(1), candidate(2), candidate(3), candidate(4)],
      start_game_music: ({ id }: any) => ready(id),
    });
    const store = await import("./music");
    store.attachAudio(fakePort());

    await store.startShuffle();
    await settle(10);
    await store.next();
    await settle(10);
    await store.next();
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(3);

    store.prev();
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(2);
    store.prev();
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(1);

    // And the track stepped back over is still what ⏭ returns to.
    await store.next();
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(2);
  });

  /** The reason set is a level, not an edge: an unmuted preview or a running
   *  game is a state that lasts, so a track loaded WHILE it lasts has to
   *  respect it too. Recorded only on the edge, pressing ▶ over a running
   *  trailer played straight through it. */
  it("a track that loads while a reason is held stays quiet until it is withdrawn", async () => {
    backend({ start_game_music: ({ id }: any) => ready(id) });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);

    // The preview is already playing with sound when the listener presses ▶.
    store.pauseFor("video");
    expect(store.pauseReasons()).toEqual(["video"]);

    store.playTheme(game(1));
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(1);
    expect(store.musicPlaying()).toBe(false);
    expect(port.playCalls).toBe(0);

    // The video ends and hands the speakers back.
    store.resumeFrom("video");
    await settle(0);
    expect(store.musicPlaying()).toBe(true);
    expect(port.playCalls).toBe(1);
  });

  /** The backend dropped the job (a cancel, a restart) while the player was
   *  waiting on it. Nothing will reconcile the wait any more, so the bar sat
   *  on "Loading…" for the rest of the session. */
  it("a job the backend has forgotten does not strand the player", async () => {
    backend({ start_game_music: () => FETCHING, get_music_status: () => null });
    const store = await import("./music");
    store.attachAudio(fakePort());

    store.playTheme(game(1));
    await settle(10);
    expect(store.wantedTrack()?.gameId).toBe(1);

    await settle(800); // the first poll comes back empty
    expect(store.wantedTrack()).toBeNull();
    expect(store.getMusicState(1)).toBeUndefined();
  });

  /** A walk that gave up is over, and so is its blacklist - otherwise the ids
   *  it tried while the swarm was quiet stay filtered out of every later
   *  shuffle. Duds here are read errors, which the "none" index does not
   *  remember, so `triedThisWalk` is the only thing that could exclude them. */
  it("a stopped walk lets a later shuffle try the same picks again", async () => {
    const ERROR = { phase: "error", progress: 0, total_bytes: 0, path: null, error: "read failed" };
    let broken = true;
    backend({
      music_shuffle_candidates: () => [candidate(20), candidate(21), candidate(22), candidate(23), candidate(24)],
      start_game_music: ({ id }: any) => (broken ? ERROR : ready(id)),
    });
    const store = await import("./music");
    store.attachAudio(fakePort());

    await store.startShuffle();
    await settle(2000);
    expect(store.currentTrack()).toBeNull();
    expect(store.musicNone().has(20)).toBe(false);

    broken = false;
    await store.startShuffle();
    await settle(10);
    expect(store.currentTrack()?.gameId).toBe(20);
  });

  /** Nothing the platform can decode means no fetch was started and none ever
   *  will be, so the queue must not stay armed on a walk that cannot move. */
  it("a refused fetch stands the queue down", async () => {
    list.rows = [row(1), row(2)];
    backend({ music_playback_supported: () => ({ mp3: false, ogg: false }) });
    const store = await import("./music");
    store.attachAudio(fakePort());

    store.playFromList(row(1));
    await settle(10);

    expect(store.wantedTrack()).toBeNull();
    expect(store.musicMode()).toBe("theme");
    expect(calls("start_game_music").length).toBe(0);
  });

  /** Bytes on disk say nothing about whether the webview can decode them, so
   *  the codec probe has to be asked before the cache. */
  it("offers no play button for a cached track the system cannot decode", async () => {
    backend({
      music_playback_supported: () => ({ mp3: false, ogg: true }),
      music_cache_index: () => ({ cached: [4], none: [] }),
    });
    const store = await import("./music");

    await store.requestTheme(4);      // learns the support answer
    await store.refreshMusicIndex();  // ...and that the track is on disk
    expect(store.musicCached().has(4)).toBe(true);

    expect(store.playableHint(row(4))).toBe(false);
  });

  it("stop clears the player and forgets the reasons", async () => {
    backend({ start_game_music: ({ id }: any) => ready(id) });
    const store = await import("./music");
    const port = fakePort();
    store.attachAudio(port);
    store.playTheme(game(1));
    await settle(10);
    store.pauseFor("game");

    store.stop();

    expect(store.currentTrack()).toBeNull();
    expect(port.src).toBeNull();
    expect(store.pauseReasons()).toEqual([]);
  });
});
