import { describe, it, expect, beforeEach } from "vitest";
import { requestSlot, releaseSlot, dropQueued, activeCount, queuedCount, isActive, isQueued, resetMediaQueue, type MediaJob } from "./mediaQueue";

function job(key: string, priority: number, log: string[]): MediaJob {
  return {
    key,
    priority: () => priority,
    run: () => { log.push(`run ${key}`); },
    onEvicted: () => { log.push(`evict ${key}`); },
    onQueued: () => { log.push(`queue ${key}`); },
  };
}

describe("media queue", () => {
  beforeEach(() => resetMediaQueue());

  // Three background videos hold every slot; the track the player waits on
  // must not sit behind them - but the game on screen still comes first.
  it("lets the wanted track evict a background video, never the foreground one", async () => {
    const log: string[] = [];
    await requestSlot(job("v:1", 0, log)); // foreground video
    await requestSlot(job("v:2", 2, log));
    await requestSlot(job("v:3", 2, log));

    await requestSlot(job("m:9", 1, log));

    expect(log).toEqual(["run v:1", "run v:2", "run v:3", "evict v:2", "run m:9"]);
    expect(isActive("v:1")).toBe(true);
    expect(isQueued("v:2")).toBe(true);
    expect(activeCount("m:")).toBe(1);
  });

  it("queues a prefetch behind equals instead of evicting them", async () => {
    const log: string[] = [];
    for (const k of ["v:1", "v:2", "v:3"]) { await requestSlot(job(k, 2, log)); }

    const result = await requestSlot(job("m:9", 2, log));

    expect(result).toBe("queued");
    expect(log[log.length - 1]).toBe("queue m:9");
    expect(queuedCount()).toBe(1);
  });

  // The player gave up on this track (skipped, stopped). Cancelling the
  // backend job is not enough: a job left in the queue starts the moment a
  // slot frees and streams bytes nobody will hear.
  it("never starts a job that was dropped from the queue", async () => {
    const log: string[] = [];
    for (const k of ["v:1", "v:2", "v:3"]) { await requestSlot(job(k, 2, log)); }
    await requestSlot(job("m:9", 2, log));
    expect(isQueued("m:9")).toBe(true);

    dropQueued("m:9");
    releaseSlot("v:1");

    expect(log).not.toContain("run m:9");
    expect(queuedCount()).toBe(0);
  });

  it("starts the most important waiting job when a slot frees", async () => {
    const log: string[] = [];
    for (const k of ["v:1", "v:2", "v:3"]) { await requestSlot(job(k, 2, log)); }
    await requestSlot(job("m:8", 2, log)); // prefetch, queued first
    await requestSlot(job("v:4", 2, log)); // background video, queued second
    // Its priority rises after queuing (the panel opened on it).
    const wanted = job("m:9", 2, log);
    await requestSlot(wanted);
    wanted.priority = () => 1;

    releaseSlot("v:1");

    expect(log[log.length - 1]).toBe("run m:9");
    expect(queuedCount()).toBe(2);
  });
});
