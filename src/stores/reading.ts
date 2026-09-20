import { createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import {
  listPublications,
  listIssues,
  readingCacheIndex,
  openIssue as openIssueCmd,
  installIssue as installIssueCmd,
  launchIssue as launchIssueCmd,
  getIssueStatus,
  cancelIssueFetch,
  removeIssue as removeIssueCmd,
  setIssueFavorited,
  setIssuePage,
  type Issue,
  type Publication,
  type ReadingStatus,
} from "../api/tauri";
import { requestSlot, releaseSlot, dropQueued, isActive, isQueued, type MediaJob } from "./mediaQueue";
import { isOffline } from "./network";

/** The Lesesaal (§19): the catalogue is bundled, so browsing needs no network
 *  and no torrent. Only opening an issue starts a fetch, and those share the
 *  three media slots with videos and theme tracks. */

const [publications, setPublications] = createSignal<Publication[]>([]);
const [issues, setIssues] = createSignal<Issue[]>([]);
const [loaded, setLoaded] = createSignal(false);
/** A store, not a signal: one tick writes one issue, and a whole-record read
 *  recomputes every row the room is showing. */
const [fetchTable, setFetchTable] = createStore<Record<string, ReadingStatus | undefined>>({});
const fetches = () => fetchTable;
/** Issues whose document is on disk - kept live so a finished fetch flips the
 *  card without a catalogue reload. */
const [onDisk, setOnDisk] = createSignal<Set<string>>(new Set());
export { publications, issues, loaded, fetches, onDisk };

export const isOnDisk = (key: string) => onDisk().has(key);

const POLL_MS = 700;
const intervals: Record<string, ReturnType<typeof setInterval>> = {};
/** Frontend-only phase: waiting for one of the three fetch slots. */
export const PHASE_QUEUED = "queued";

const keyOf = (issueKey: string) => `r:${issueKey}`;

/** The whole catalogue is ~1,500 rows, so it is loaded once and filtered in
 *  the client - a keystroke must not become a round trip. */
export async function loadReadingCatalog(force = false): Promise<void> {
  if (loaded() && !force) { return; }
  const [pubs, rows, cached] = await Promise.all([
    listPublications(),
    listIssues(),
    readingCacheIndex().catch(() => [] as string[]),
  ]);
  setPublications(pubs);
  setIssues(rows);
  setOnDisk(new Set(cached));
  setLoaded(true);
}

export function issueStatus(key: string): ReadingStatus | undefined {
  return fetches()[key];
}

function put(key: string, status: ReadingStatus) {
  // "none" with an error is provisional (offline): shown once, never kept, or
  // one offline visit would mark the issue unavailable for the session.
  const provisional = status.phase === "none" && status.error != null;
  setFetchTable(key, status);
  if (provisional) {
    forget(key);
    return;
  }
  if (status.phase === "ready") { landed(key); }
}

/** The document is on disk: the card flips without a catalogue reload, and a
 *  runnable issue becomes playable. Here, not in the poll, because a command
 *  can answer "ready" outright and never be polled at all. */
function landed(key: string) {
  setOnDisk((prev) => new Set(prev).add(key));
  const row = issues().find((issue) => issue.key === key);
  if (row?.runnable) { patchIssue(key, { installed: true }); }
}

function forget(key: string) {
  setFetchTable(key, undefined);
}

const queuedStatus = (): ReadingStatus =>
  ({ phase: PHASE_QUEUED, progress: 0, total_bytes: 0, path: null, error: null });

const errorStatus = (e: unknown): ReadingStatus =>
  ({ phase: "error", progress: 0, total_bytes: 0, path: null, error: String(e) });

function stopPolling(key: string) {
  const handle = intervals[key];
  if (handle) {
    clearInterval(handle);
    delete intervals[key];
  }
}

/** End the job: the interval and the slot always go together. */
function finish(key: string) {
  stopPolling(key);
  releaseSlot(keyOf(key));
}

function poll(key: string) {
  stopPolling(key);
  intervals[key] = setInterval(() => { void tick(key); }, POLL_MS);
}

async function tick(key: string) {
  let status: ReadingStatus | null;
  try {
    status = await getIssueStatus(key);
  } catch (e) {
    // The poll is the only thing that ever ends a fetch, so a rejection left
    // alone pins the card at "fetching" and burns one of the three slots.
    console.error("[reading] status poll failed:", e);
    finish(key);
    put(key, errorStatus(e));
    return;
  }
  if (!status) {
    // The backend dropped the job (a cancel landed). Leaving the last
    // "fetching" behind would freeze the card and refuse every retry.
    finish(key);
    forget(key);
    return;
  }
  put(key, status);
  if (status.phase === "ready" || status.phase === "error" || status.phase === "none") {
    finish(key);
  }
}

/** Fetch an issue, or return the one already on disk. The caller renders from
 *  `fetches()[key]`; nothing here blocks. */
export async function requestIssue(key: string): Promise<void> {
  await startFetch(key, () => openIssueCmd(key));
}

/** Download a disk magazine so it can be launched. Same job, same slots and
 *  the same polling as reading one - only the backend command differs. */
export async function installIssue(key: string): Promise<void> {
  await startFetch(key, () => installIssueCmd(key));
}

export async function launchIssue(key: string): Promise<string> {
  return launchIssueCmd(key);
}

/** Bumped whenever the job stops being the one the user asked for. A `run`
 *  that is still awaiting the backend compares against it and drops the
 *  answer, instead of reviving a fetch whose slot is already gone. */
const epochs: Record<string, number> = {};
const bumpEpoch = (key: string) => { epochs[key] = (epochs[key] ?? 0) + 1; };

/** A fetch holds its slot from the click until the backend's first answer;
 *  that window is in flight although the store may not show it yet. */
export const isFetchInFlight = (key: string) =>
  isActive(keyOf(key)) || isQueued(keyOf(key));

async function startFetch(key: string, start: () => Promise<ReadingStatus>): Promise<void> {
  if (isActive(keyOf(key))) { return; }
  const current = issueStatus(key);
  if (current?.phase === "ready" || current?.phase === "fetching") { return; }
  if (isOffline()) {
    put(key, { phase: "none", progress: 0, total_bytes: 0, path: null, error: "offline" });
    return;
  }
  // The first answer can be seconds away; without this the click leaves the
  // card looking untouched until then.
  put(key, queuedStatus());

  const job: MediaJob = {
    key: keyOf(key),
    // The reader is open on it and the user is waiting - same rank as the
    // visible game's preview.
    priority: () => 0,
    run: async () => {
      const epoch = epochs[key] ?? 0;
      const stale = () => (epochs[key] ?? 0) !== epoch;
      try {
        const status = await start();
        if (stale()) { return; }
        put(key, status);
        if (status.phase === "fetching") {
          poll(key);
        } else {
          releaseSlot(keyOf(key));
        }
      } catch (e) {
        if (stale()) { return; }
        put(key, errorStatus(e));
        releaseSlot(keyOf(key));
      }
    },
    onEvicted: () => {
      bumpEpoch(key);
      void cancelIssueFetch(key).catch((e) => console.error("[reading] cancel failed:", e));
      stopPolling(key);
      forget(key);
    },
    onQueued: () => {
      put(key, queuedStatus());
    },
  };
  await requestSlot(job);
}

export async function abortIssue(key: string): Promise<void> {
  bumpEpoch(key);
  stopPolling(key);
  // Out of the queue as well: a job that never got a slot has nothing to
  // cancel in the backend and would start later, unasked.
  dropQueued(keyOf(key));
  releaseSlot(keyOf(key));
  forget(key);
  await cancelIssueFetch(key).catch((e) => console.error("[reading] cancel failed:", e));
}

// ── Presentation order ───────────────────────────────────────────────────────

export type ReadingSort =
  | "publication" | "title" | "title_desc" | "year_desc" | "year_asc" | "size" | "size_desc";

const byText = (a: string, b: string) =>
  a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" });
const titleOf = (issue: Issue) => issue.sort_title ?? issue.title;
// ISO dates compare as strings; an unknown date sorts after every known one.
const byDate = (a: Issue, b: Issue) => byText(a.release_date ?? "\uffff", b.release_date ?? "\uffff");
// An unknown year sorts last in either direction.
const byYear = (a: Issue, b: Issue, desc = false) => {
  if (a.year == null || b.year == null) { return a.year == null ? (b.year == null ? 0 : 1) : -1; }
  return desc ? b.year - a.year : a.year - b.year;
};

const COMPARATORS: Record<ReadingSort, (a: Issue, b: Issue) => number> = {
  publication: (a, b) =>
    byText(a.publication, b.publication) || byDate(a, b) || byText(titleOf(a), titleOf(b)),
  title: (a, b) => byText(titleOf(a), titleOf(b)) || byDate(a, b),
  title_desc: (a, b) => byText(titleOf(b), titleOf(a)) || byDate(a, b),
  year_asc: (a, b) => byYear(a, b) || byDate(a, b) || byText(titleOf(a), titleOf(b)),
  year_desc: (a, b) => byYear(a, b, true) || byDate(b, a) || byText(titleOf(a), titleOf(b)),
  size: (a, b) => a.size_bytes - b.size_bytes || byText(titleOf(a), titleOf(b)),
  size_desc: (a, b) => b.size_bytes - a.size_bytes || byText(titleOf(a), titleOf(b)),
};

export function sortIssues(rows: Issue[], sort: ReadingSort): Issue[] {
  return [...rows].sort(COMPARATORS[sort]);
}

/** Section label under `sort`; "" where the order has no natural grouping. */
export function sectionOf(issue: Issue, sort: ReadingSort): string {
  switch (sort) {
    case "publication":
      return issue.publication;
    case "title":
    case "title_desc": {
      const first = titleOf(issue)[0]?.toUpperCase() ?? "";
      return /[A-Z]/.test(first) ? first : "#";
    }
    case "year_asc":
    case "year_desc":
      return issue.year != null ? String(issue.year) : "Unknown";
    default:
      return "";
  }
}

/** eXo titles repeat the series ("PC World: Issue 22"); with the publication
 *  already in view that prefix is noise, without it the full title stays. */
export function displayTitle(issue: Issue, publicationInView: boolean): string {
  if (!publicationInView) { return issue.title; }
  const prefix = `${issue.publication.toLowerCase()}:`;
  if (!issue.title.toLowerCase().startsWith(prefix)) { return issue.title; }
  return issue.title.slice(prefix.length).trim() || issue.title;
}

export function kindLabel(issue: Issue): string {
  if (issue.runnable) { return "Disk magazine"; }
  return issue.kind.charAt(0).toUpperCase() + issue.kind.slice(1);
}

// ── Filters ─────────────────────────────────────────────────────────────────

export type Kind = "all" | "magazine" | "book" | "catalog";
export type ReadingLanguage = "all" | "EN" | "DE";

export const KINDS: { id: Kind; label: string }[] = [
  { id: "all", label: "Everything" },
  { id: "magazine", label: "Magazines" },
  { id: "book", label: "Books" },
  { id: "catalog", label: "Catalogs" },
];

export const LANGUAGES: { id: ReadingLanguage; label: string }[] = [
  { id: "all", label: "All" },
  { id: "EN", label: "English" },
  { id: "DE", label: "Deutsch" },
];

export interface IssueFilter {
  kind: Kind;
  language: ReadingLanguage;
  publicationId: number | null;
  query: string;
}

const matchesQuery = (issue: Issue, needle: string) =>
  issue.title.toLowerCase().includes(needle) ||
  issue.publication.toLowerCase().includes(needle) ||
  String(issue.year ?? "").includes(needle);

export function filterIssues(rows: Issue[], filter: IssueFilter): Issue[] {
  const needle = filter.query.trim().toLowerCase();
  return rows.filter(
    (issue) =>
      (filter.kind === "all" || issue.kind === filter.kind) &&
      (filter.language === "all" || issue.language === filter.language) &&
      (filter.publicationId == null || issue.publication_id === filter.publicationId) &&
      (needle === "" || matchesQuery(issue, needle)),
  );
}

export interface PublicationGroup {
  /** Section header; null where the filter already implies the group. */
  label: string | null;
  rows: Publication[];
}

/** Publications under the active kind and language, in select order. The
 *  German series form one group of their own only while both languages are
 *  in view; under a single language they sit in their kind like any other. */
export function publicationGroups(rows: Publication[], kind: Kind, language: ReadingLanguage): PublicationGroup[] {
  const byName = (a: Publication, b: Publication) => a.name.localeCompare(b.name);
  const inLanguage = rows.filter((p) => language === "all" || p.language === language);
  const german = language === "all" ? inLanguage.filter((p) => p.language === "DE") : [];
  const rest = language === "all" ? inLanguage.filter((p) => p.language !== "DE") : inLanguage;
  const groups: PublicationGroup[] = [];
  const kinds = kind === "all" ? KINDS.filter((k) => k.id !== "all") : KINDS.filter((k) => k.id === kind);
  for (const k of kinds) {
    const inKind = rest.filter((p) => p.kind === k.id).sort(byName);
    if (inKind.length === 0) { continue; }
    groups.push({ label: kind === "all" ? k.label : null, rows: inKind });
  }
  if (german.length > 0) {
    groups.push({ label: "Deutsch", rows: german.sort(byName) });
  }
  return groups;
}

function patchIssue(key: string, patch: Partial<Issue>) {
  setIssues((rows) => rows.map((row) => (row.key === key ? { ...row, ...patch } : row)));
}

/** Give an issue's disk space back. Nothing sweeps the reading room, so this
 *  is the only way a download leaves the disk - and the only thing that takes
 *  a key out of `onDisk`. */
export async function removeIssue(issue: Issue): Promise<void> {
  await removeIssueCmd(issue.key);
  setOnDisk((prev) => {
    const next = new Set(prev);
    next.delete(issue.key);
    return next;
  });
  // A "ready" status outlives the file it points at: the reader would open a
  // path that is gone instead of offering the download again.
  forget(issue.key);
  if (issue.runnable) { patchIssue(issue.key, { installed: false }); }
}

export async function toggleIssueFavorite(issue: Issue): Promise<void> {
  const favorited = !issue.favorited;
  patchIssue(issue.key, { favorited });
  await setIssueFavorited(issue.key, favorited);
}

/** Remember the page the reader is on. Debounced: page turns are frequent and
 *  each one is a DB write. */
const pageTimers: Record<string, ReturnType<typeof setTimeout>> = {};
export function rememberPage(key: string, page: number) {
  patchIssue(key, { last_page: page });
  // Per issue: one shared timer lost the page of whichever issue the reader
  // left within the debounce window.
  clearTimeout(pageTimers[key]);
  pageTimers[key] = setTimeout(() => {
    delete pageTimers[key];
    void setIssuePage(key, page);
  }, 1000);
}
