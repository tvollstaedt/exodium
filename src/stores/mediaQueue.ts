/** Fetch slots shared by everything that streams out of a GameData archive.
 *
 *  Each fetch is a torrent stream with its own 32 MB lookahead, and several at
 *  once fight over the same peers - so only MAX_CONCURRENT run, whatever kind
 *  they are. Videos and theme tracks used to need one such scheduler each;
 *  two caps of three would have been six streams.
 *
 *  Jobs carry a priority (lower number = more important): the game on screen
 *  wants its video now (0), the track the player is waiting on comes next (1),
 *  background videos and the shuffle's prefetch wait their turn (2). A request
 *  that finds no free slot evicts the least important, oldest running job -
 *  only one that is strictly less important than itself - and otherwise
 *  queues. An evicted job is not lost: it goes back to the FRONT of the queue,
 *  and its owner is told so it can show "queued" rather than nothing.
 *
 *  Priorities are read at decision time, not at request time, because they
 *  move: the foreground game changes, the prefetched track becomes the
 *  current one. */

export interface MediaJob {
  /** Unique across kinds: `v:<gameId>` for videos, `m:<gameId>` for music. */
  key: string;
  /** Read whenever the scheduler decides; lower is more important. */
  priority: () => number;
  /** Start the work. Called by the scheduler once a slot is held. */
  run: () => void | Promise<void>;
  /** The slot was taken away; the job is back in the queue. */
  onEvicted: () => void;
  /** The job had to wait for a slot. */
  onQueued: () => void;
}

export const MAX_CONCURRENT = 3;

/** Running jobs, oldest first - the eviction order within a priority. */
let active: MediaJob[] = [];
/** Waiting jobs, next one first. */
let queue: MediaJob[] = [];

export function isActive(key: string): boolean {
  return active.some((j) => j.key === key);
}

export function isQueued(key: string): boolean {
  return queue.some((j) => j.key === key);
}

export function activeCount(prefix = ""): number {
  return active.filter((j) => j.key.startsWith(prefix)).length;
}

export function queuedCount(prefix = ""): number {
  return queue.filter((j) => j.key.startsWith(prefix)).length;
}

/** Take a job out of the queue without running it. */
export function dropQueued(key: string) {
  queue = queue.filter((j) => j.key !== key);
}

/** The least important running job that matters less than `priority`, oldest
 *  first among equals. Undefined when nothing may be evicted for it. */
function evictionVictim(priority: number): MediaJob | undefined {
  let victim: MediaJob | undefined;
  for (const job of active) {
    const p = job.priority();
    if (p <= priority) { continue; }
    if (!victim || p > victim.priority()) { victim = job; }
  }
  return victim;
}

function evict(job: MediaJob) {
  active = active.filter((j) => j !== job);
  job.onEvicted();
  if (!queue.some((j) => j.key === job.key)) { queue.unshift(job); }
}

async function start(job: MediaJob): Promise<void> {
  active.push(job);
  await job.run();
}

/** Run the job now if a slot is free (or can be taken from something less
 *  important), otherwise queue it. Resolves once `run` has returned, so a
 *  caller can rely on the job's first status being in place. */
export async function requestSlot(job: MediaJob): Promise<"running" | "queued"> {
  if (isActive(job.key)) { return "running"; }
  dropQueued(job.key);
  if (active.length >= MAX_CONCURRENT) {
    const victim = evictionVictim(job.priority());
    if (!victim) {
      job.onQueued();
      queue.push(job);
      return "queued";
    }
    evict(victim);
  }
  await start(job);
  return "running";
}

/** The job finished (or failed, or was cancelled). Frees its slot and starts
 *  whatever is waiting - most important first, then in arrival order. */
export function releaseSlot(key: string) {
  active = active.filter((j) => j.key !== key);
  pump();
}

function pump() {
  while (queue.length > 0 && active.length < MAX_CONCURRENT) {
    let best = 0;
    for (let i = 1; i < queue.length; i++) {
      if (queue[i].priority() < queue[best].priority()) { best = i; }
    }
    const [next] = queue.splice(best, 1);
    void start(next);
  }
}

/** Tests only: forget every job. */
export function resetMediaQueue() {
  active = [];
  queue = [];
}
