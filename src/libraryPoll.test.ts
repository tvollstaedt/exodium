import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createLibraryPoll } from "./libraryPoll";

/** Resolves with the next queued answer once the test releases it. */
function slowFingerprint() {
  const waiting: Array<(v: string) => void> = [];
  const fn = vi.fn(() => new Promise<string>((resolve) => { waiting.push(resolve); }));
  return { fn, answer: (v: string) => waiting.shift()?.(v) };
}

describe("createLibraryPoll", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("fetches the lists only when the fingerprint moved", async () => {
    let fp = "a";
    const refresh = vi.fn();
    const poll = createLibraryPoll({ fingerprint: async () => fp, refresh });
    await poll.prime();
    poll.start();

    await vi.advanceTimersByTimeAsync(15000);
    expect(refresh).not.toHaveBeenCalled();

    fp = "b";
    await vi.advanceTimersByTimeAsync(5000);
    expect(refresh).toHaveBeenCalledTimes(1);

    await vi.advanceTimersByTimeAsync(10000);
    expect(refresh).toHaveBeenCalledTimes(1);
    poll.stop();
  });

  it("refreshes on the first tick when it was never primed", async () => {
    const refresh = vi.fn();
    const poll = createLibraryPoll({ fingerprint: async () => "a", refresh });
    poll.start();
    await vi.advanceTimersByTimeAsync(5000);
    expect(refresh).toHaveBeenCalledTimes(1);
    poll.stop();
  });

  it("does not ask again while the previous answer is outstanding", async () => {
    const { fn, answer } = slowFingerprint();
    const poll = createLibraryPoll({ fingerprint: fn, refresh: vi.fn() });
    poll.start();

    await vi.advanceTimersByTimeAsync(25000);
    expect(fn).toHaveBeenCalledTimes(1);

    answer("a");
    await vi.advanceTimersByTimeAsync(5000);
    expect(fn).toHaveBeenCalledTimes(2);
    poll.stop();
  });

  it("stays quiet while paused and catches up once it resumes", async () => {
    let fp = "a";
    let paused = true;
    const refresh = vi.fn();
    const fingerprint = vi.fn(async () => fp);
    const poll = createLibraryPoll({ fingerprint, refresh, paused: () => paused });
    await poll.prime();
    fingerprint.mockClear();
    poll.start();

    fp = "b";
    await vi.advanceTimersByTimeAsync(20000);
    expect(fingerprint).not.toHaveBeenCalled();

    paused = false;
    await vi.advanceTimersByTimeAsync(5000);
    expect(refresh).toHaveBeenCalledTimes(1);
    poll.stop();
  });

  it("survives a failed fingerprint and keeps ticking", async () => {
    let fail = true;
    const refresh = vi.fn();
    const poll = createLibraryPoll({
      fingerprint: async () => { if (fail) { throw new Error("db busy"); } return "a"; },
      refresh,
    });
    poll.start();
    await vi.advanceTimersByTimeAsync(5000);
    expect(refresh).not.toHaveBeenCalled();

    fail = false;
    await vi.advanceTimersByTimeAsync(5000);
    expect(refresh).toHaveBeenCalledTimes(1);
    poll.stop();
  });
});
