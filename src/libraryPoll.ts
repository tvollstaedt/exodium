/** Re-fetches the Library shelves only when the library changed. Their lists
 *  run to megabytes on a big library, a fingerprint is one short string. */
export function createLibraryPoll(opts: {
  fingerprint: () => Promise<string>;
  refresh: () => void;
  /** True while nobody looks (window without focus): no tick, no cost. */
  paused?: () => boolean;
  intervalMs?: number;
}) {
  let seen: string | null = null;
  let busy = false;
  let timer: ReturnType<typeof setInterval> | undefined;

  /** Take the library's current state. Call it BEFORE fetching the lists, so
   *  a change that lands during the fetch still shows on the next tick. */
  const prime = async () => {
    try {
      seen = await opts.fingerprint();
    } catch {
      // Unchanged `seen`: the next tick compares against the older state.
    }
  };

  // A slow answer must not queue up the next tick behind itself.
  const tick = async () => {
    if (busy || opts.paused?.()) { return; }
    busy = true;
    try {
      const now = await opts.fingerprint();
      if (now !== seen) {
        seen = now;
        opts.refresh();
      }
    } catch {
      // The next tick asks again.
    } finally {
      busy = false;
    }
  };

  return {
    prime,
    start: () => { timer ??= setInterval(() => { void tick(); }, opts.intervalMs ?? 5000); },
    stop: () => { if (timer !== undefined) { clearInterval(timer); timer = undefined; } },
  };
}
