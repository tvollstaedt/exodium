import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("./games", () => ({ games: () => [], hasMore: () => false, fetchMoreGames: vi.fn(async () => {}) }));
vi.mock("./network", () => ({ isOffline: () => false, networkMode: () => "live" }));

/** A fake <video>: records what the controller asked of it and lets the
 *  test settle or refuse a `play()` by hand. */
function fakeVideo() {
  const handlers: Record<string, (() => void) | ((m: string) => void)> = {};
  let resolvePlay: (() => void) | undefined;
  let rejectPlay: ((e: unknown) => void) | undefined;
  const port = {
    src: null as string | null,
    muted: false,
    playCalls: 0,
    pauseCalls: 0,
    seeks: 0,
    setSrc(url: string | null) { port.src = url; },
    play: () => {
      port.playCalls++;
      return new Promise<void>((res, rej) => { resolvePlay = res; rejectPlay = rej; });
    },
    pause: () => { port.pauseCalls++; },
    setMuted(m: boolean) { port.muted = m; },
    isMuted: () => port.muted,
    volume: 1,
    setVolume(v: number) { port.volume = v; },
    reloads: 0,
    reload() { port.reloads++; },
    seekStart() { port.seeks++; },
    onPlay(cb: () => void) { handlers.play = cb; },
    onPause(cb: () => void) { handlers.pause = cb; },
    onEnded(cb: () => void) { handlers.ended = cb; },
    onError(cb: (m: string) => void) { handlers.error = cb; },
    /** The element started rendering frames. */
    started() { resolvePlay?.(); (handlers.play as () => void)?.(); },
    refused(name: string) { rejectPlay?.(new DOMException("no", name)); },
    paused() { (handlers.pause as () => void)?.(); },
    ended() { (handlers.ended as () => void)?.(); },
    errored(m: string) { (handlers.error as (m: string) => void)?.(m); },
  };
  return port;
}

async function settle(ms = 0) {
  await vi.advanceTimersByTimeAsync(ms);
}

describe("hero video controller", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.useFakeTimers();
  });
  afterEach(() => vi.useRealTimers());

  async function boot() {
    const hero = await import("./heroVideo");
    const music = await import("./music");
    const port = fakeVideo();
    hero.attachVideo(port);
    return { hero, music, port };
  }

  it("waits out the cover beat, then plays; unmuted it claims the speakers on scheduling", async () => {
    const { hero, music, port } = await boot();
    hero.showPreview(1, "v1", { muted: false, delayMs: 2000 });
    expect(hero.heroPhase()).toBe("loading");
    expect(music.pauseReasons()).toEqual(["video"]);
    expect(port.playCalls).toBe(0);
    await settle(2000);
    expect(port.src).toBe("v1");
    expect(port.playCalls).toBe(1);
    expect(port.muted).toBe(false);
    port.started();
    await settle(0);
    expect(hero.heroPlayingFor(1)).toBe(true);
    port.ended();
    expect(hero.heroPhase()).toBe("ended");
    expect(music.pauseReasons()).toEqual([]);
  });

  it("a start still pending when the panel moves on is cancelled, and the element unloaded", async () => {
    const { hero, music, port } = await boot();
    hero.showPreview(1, "v1", { muted: false, delayMs: 0 });
    await settle(0);
    expect(port.playCalls).toBe(1);
    // The next game has no video: the element must hold nothing it could
    // start on later.
    hero.clearPreview();
    expect(port.pauseCalls).toBe(1);
    expect(port.src).toBeNull();
    expect(music.pauseReasons()).toEqual([]);
    // The old start settles late: it must not resurrect anything.
    port.started();
    await settle(0);
    expect(hero.heroPhase()).toBe("idle");
    expect(hero.heroPlayingFor(1)).toBe(false);
  });

  it("switching to another game's video restarts cleanly under a new sequence", async () => {
    const { hero, port } = await boot();
    hero.showPreview(1, "v1", { muted: true, delayMs: 0 });
    await settle(0);
    hero.showPreview(2, "v2", { muted: true, delayMs: 0 });
    // The first play's refusal lands after the switch: ignored.
    port.refused("NotSupportedError");
    await settle(0);
    expect(hero.heroError()).toBeNull();
    expect(hero.heroGameId()).toBe(2);
    expect(port.src).toBe("v2");
    expect(port.playCalls).toBe(2);
  });

  it("an unmuted start the engine refuses is retried muted, and the speakers go back", async () => {
    const { hero, music, port } = await boot();
    hero.showPreview(1, "v1", { muted: false, delayMs: 0 });
    await settle(0);
    expect(music.pauseReasons()).toEqual(["video"]);
    port.refused("NotAllowedError");
    await settle(0);
    expect(port.playCalls).toBe(2);
    expect(port.muted).toBe(true);
    expect(music.pauseReasons()).toEqual([]);
    port.refused("NotAllowedError");
    await settle(0);
    expect(hero.heroPhase()).toBe("failed");
    expect(hero.heroError()).toContain("NotAllowedError");
  });

  it("re-running the caller with the same game and source does not restart it", async () => {
    const { hero, port } = await boot();
    hero.showPreview(1, "v1", { muted: true, delayMs: 0 });
    await settle(0);
    port.started();
    hero.showPreview(1, "v1", { muted: true, delayMs: 0 });
    await settle(0);
    expect(port.playCalls).toBe(1);
  });

  it("detaching the element stops it and gives the speakers back", async () => {
    const { hero, music, port } = await boot();
    hero.showPreview(1, "v1", { muted: false, delayMs: 0 });
    await settle(0);
    port.started();
    expect(music.pauseReasons()).toEqual(["video"]);
    hero.attachVideo(null);
    expect(port.pauseCalls).toBe(1);
    expect(port.src).toBeNull();
    expect(music.pauseReasons()).toEqual([]);
    expect(hero.heroPhase()).toBe("idle");
  });

  it("the lightbox pauses the hero and keeps the claim only while it has the trailer with sound", async () => {
    const { hero, music, port } = await boot();
    hero.showPreview(1, "v1", { muted: false, delayMs: 0 });
    await settle(0);
    port.started();
    hero.setLightbox(true, true);
    expect(port.pauseCalls).toBe(1);
    port.paused();
    expect(hero.heroPhase()).toBe("paused");
    expect(music.pauseReasons()).toEqual(["video"]);
    hero.setLightbox(false, false);
    expect(music.pauseReasons()).toEqual([]);
  });

  it("a media error gets one silent reload per game, the second one is named", async () => {
    const { hero, port } = await boot();
    hero.showPreview(1, "v1", { muted: true, delayMs: 0 });
    await settle(0);
    port.errored("MediaError 4");
    expect(hero.heroPhase()).toBe("loading");
    expect(hero.heroError()).toBeNull();
    await settle(800);
    expect(port.reloads).toBe(1);
    expect(port.playCalls).toBe(2);
    port.errored("MediaError 4");
    expect(hero.heroPhase()).toBe("failed");
    expect(hero.heroError()).toBe("MediaError 4");
  });

  it("an unmuted start ramps the volume in", async () => {
    const { hero, port } = await boot();
    hero.showPreview(1, "v1", { muted: false, delayMs: 0 });
    await settle(0);
    expect(port.volume).toBe(0);
    await settle(700);
    expect(port.volume).toBe(1);
  });

  it("unmuting a paused preview restarts it with sound; muting releases the speakers", async () => {
    const { hero, music, port } = await boot();
    hero.showPreview(1, "v1", { muted: true, delayMs: 0 });
    await settle(0);
    port.started();
    expect(music.pauseReasons()).toEqual([]);
    hero.setPreviewMutedNow(false);
    expect(music.pauseReasons()).toEqual(["video"]);
    hero.setPreviewMutedNow(true);
    expect(music.pauseReasons()).toEqual([]);
    port.paused();
    hero.setPreviewMutedNow(false);
    expect(port.playCalls).toBe(2);
    expect(port.muted).toBe(false);
  });
});
