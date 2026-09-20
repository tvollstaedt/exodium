import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import type { Issue, Publication } from "../api/tauri";

const mockInvoke = vi.mocked(invoke);

const FETCHING = { phase: "fetching", progress: 0.1, total_bytes: 141_000_000, path: null, error: null };
const READY = { phase: "ready", progress: 1, total_bytes: 141_000_000, path: "/root/eXo/Magazines/BBD", error: null };

const DISK_MAGAZINE = {
  id: 1,
  key: "mag:eXo/Magazines/BBD#004",
  publication_id: 1,
  publication: "Big Blue Disk",
  kind: "magazine",
  title: "Big Blue Disk 004",
  sort_title: null,
  year: 1986,
  release_date: null,
  publisher: null,
  developer: null,
  notes: null,
  source: "eXoMedia",
  zip_file: "Content/DOSMagazines.zip",
  inner_zip: null,
  entry_path: null,
  entry_kind: null,
  size_bytes: 141_000_000,
  cover_key: null,
  runnable: true,
  launch_dir: "eXo/Magazines/BBD",
  issue_dir: null,
  launch_bat: "Big Blue Disk.bat",
  command_line: "004",
  substitutions: '{"XXX":"004","ZZZ":"3000"}',
  language: "EN",
  extras_count: 0,
  favorited: false,
  installed: false,
  last_page: null,
  last_opened: null,
};

/** A readable issue: one cached file, no launcher. */
const DOCUMENT: Issue = {
  ...DISK_MAGAZINE,
  id: 2,
  key: "mag:eXo/Magazines/PCGamerUS#1995_02",
  publication: "PC Gamer",
  title: "PC Gamer 1995-02",
  entry_path: "eXo/Magazines/PCGamerUS/PCGamer_1995_02.pdf",
  entry_kind: "pdf",
  size_bytes: 57_000_000,
  runnable: false,
  launch_dir: null,
  launch_bat: null,
  command_line: null,
  substitutions: null,
};

function backend(handlers: Record<string, (args: any) => any>) {
  mockInvoke.mockImplementation(async (cmd: string, args: any) => {
    const h = handlers[cmd];
    return h ? h(args ?? {}) : null;
  });
}

const calls = (cmd: string) =>
  mockInvoke.mock.calls.filter((c) => c[0] === cmd).map((c) => c[1] as any);

describe("reading store", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    vi.resetModules();
    vi.useFakeTimers();
  });
  afterEach(() => vi.useRealTimers());

  // The card turns from "download" into "play" on the install finishing, and
  // nothing reloads the catalogue in between.
  it("marks a disk magazine installed when its fetch completes", async () => {
    backend({
      list_publications: () => [],
      list_issues: () => [DISK_MAGAZINE],
      install_issue: () => FETCHING,
      get_issue_status: () => READY,
    });
    const store = await import("./reading");
    await store.loadReadingCatalog();

    await store.installIssue(DISK_MAGAZINE.key);
    expect(store.issueStatus(DISK_MAGAZINE.key)?.phase).toBe("fetching");
    await vi.advanceTimersByTimeAsync(1000);

    expect(calls("install_issue").length).toBe(1);
    expect(store.issues()[0].installed).toBe(true);
  });

  // Offline is a state, not a verdict (§14): the answer is shown once and
  // never kept, or one offline visit marks the issue unavailable all session.
  it("does not remember an offline answer", async () => {
    backend({
      list_publications: () => [],
      list_issues: () => [DISK_MAGAZINE],
      get_config: () => "offline",
    });
    const network = await import("./network");
    const store = await import("./reading");
    await network.loadNetworkMode();
    await store.loadReadingCatalog();

    await store.installIssue(DISK_MAGAZINE.key);
    expect(calls("install_issue").length).toBe(0);
    expect(store.issueStatus(DISK_MAGAZINE.key)).toBeUndefined();
  });

  // A job that never got one of the three slots must leave the queue when the
  // user gives up, or it starts a transfer nobody asked for any more.
  it("takes a cancelled fetch out of the slot queue", async () => {
    backend({
      list_publications: () => [],
      list_issues: () => [DISK_MAGAZINE],
      install_issue: () => FETCHING,
      get_issue_status: () => FETCHING,
      cancel_issue_fetch: () => null,
    });
    const queue = await import("./mediaQueue");
    const store = await import("./reading");
    queue.resetMediaQueue();
    await store.loadReadingCatalog();

    // Fill every slot with something more important than nothing.
    for (const key of ["v:1", "v:2", "v:3"]) {
      await queue.requestSlot({ key, priority: () => 0, run: () => {}, onEvicted: () => {}, onQueued: () => {} });
    }
    await store.installIssue(DISK_MAGAZINE.key);
    expect(queue.isQueued(`r:${DISK_MAGAZINE.key}`)).toBe(true);

    await store.abortIssue(DISK_MAGAZINE.key);
    expect(queue.isQueued(`r:${DISK_MAGAZINE.key}`)).toBe(false);
    queue.releaseSlot("v:1");
    expect(calls("install_issue").length).toBe(0);
  });

  // Nothing sweeps the reading room any more, so the card has to follow a
  // removal: out of the on-disk set, and a disk magazine out of "installed".
  it("drops a removed issue from the on-disk set", async () => {
    backend({
      list_publications: () => [],
      list_issues: () => [DOCUMENT, DISK_MAGAZINE],
      reading_cache_index: () => [DOCUMENT.key],
      install_issue: () => FETCHING,
      get_issue_status: () => READY,
      remove_issue: () => null,
    });
    const store = await import("./reading");
    await store.loadReadingCatalog();
    expect(store.isOnDisk(DOCUMENT.key)).toBe(true);

    await store.removeIssue(DOCUMENT);
    expect(calls("remove_issue")[0].key).toBe(DOCUMENT.key);
    expect(store.isOnDisk(DOCUMENT.key)).toBe(false);

    await store.installIssue(DISK_MAGAZINE.key);
    await vi.advanceTimersByTimeAsync(1000);
    expect(store.issues()[1].installed).toBe(true);

    await store.removeIssue(store.issues()[1]);
    expect(store.issues()[1].installed).toBe(false);
    // The "ready" status points at a path that is gone; keeping it would
    // reopen the reader on it instead of offering the download again.
    expect(store.issueStatus(DISK_MAGAZINE.key)).toBeUndefined();
  });

  // A refused open (the GLP torrent is not part of this install, say) stays
  // on the card as an error with "Try again"; the retry asks the backend
  // again rather than serving the remembered refusal.
  it("keeps an open_issue error and asks again on the next request", async () => {
    backend({
      list_publications: () => [],
      list_issues: () => [DOCUMENT],
      open_issue: () => { throw new Error("the eXoDOS_GLP torrent is not enabled"); },
    });
    const store = await import("./reading");
    await store.loadReadingCatalog();

    await store.requestIssue(DOCUMENT.key);
    expect(store.issueStatus(DOCUMENT.key)?.phase).toBe("error");
    expect(store.issueStatus(DOCUMENT.key)?.error).toContain("eXoDOS_GLP");

    await store.requestIssue(DOCUMENT.key);
    expect(calls("open_issue").length).toBe(2);
    expect(store.issueStatus(DOCUMENT.key)?.phase).toBe("error");
  });

  // The poll is the only thing that ends a fetch: a rejected status call left
  // the card at "fetching" and one of the three slots occupied for good.
  it("survives a failing get_issue_status", async () => {
    backend({
      list_publications: () => [],
      list_issues: () => [DISK_MAGAZINE],
      install_issue: () => FETCHING,
      get_issue_status: () => { throw new Error("db locked"); },
    });
    const queue = await import("./mediaQueue");
    const store = await import("./reading");
    queue.resetMediaQueue();
    await store.loadReadingCatalog();

    await store.installIssue(DISK_MAGAZINE.key);
    await vi.advanceTimersByTimeAsync(1000);

    expect(store.issueStatus(DISK_MAGAZINE.key)?.phase).toBe("error");
    expect(queue.activeCount("r:")).toBe(0);
    // The interval is gone too, not just silent.
    const polls = calls("get_issue_status").length;
    await vi.advanceTimersByTimeAsync(3000);
    expect(calls("get_issue_status").length).toBe(polls);
  });

  // The slot is the scarce resource: three at a time for the whole app.
  it("releases its media slot when a fetch finishes", async () => {
    backend({
      list_publications: () => [],
      list_issues: () => [DISK_MAGAZINE, DOCUMENT],
      install_issue: () => FETCHING,
      get_issue_status: () => READY,
      open_issue: () => { throw new Error("no such entry"); },
    });
    const queue = await import("./mediaQueue");
    const store = await import("./reading");
    queue.resetMediaQueue();
    await store.loadReadingCatalog();

    await store.installIssue(DISK_MAGAZINE.key);
    expect(queue.activeCount("r:")).toBe(1);
    await vi.advanceTimersByTimeAsync(1000);
    expect(queue.activeCount("r:")).toBe(0);

    await store.requestIssue(DOCUMENT.key);
    expect(store.issueStatus(DOCUMENT.key)?.phase).toBe("error");
    expect(queue.activeCount("r:")).toBe(0);
  });

  // The command can answer long after the user gave up; putting that answer
  // would resurrect a card and poll a job whose slot is already gone.
  it("ignores a status that lands after an abort", async () => {
    let answer: (status: unknown) => void = () => {};
    backend({
      list_publications: () => [],
      list_issues: () => [DOCUMENT],
      open_issue: () => new Promise((resolve) => { answer = resolve; }),
      get_issue_status: () => FETCHING,
      cancel_issue_fetch: () => null,
    });
    const queue = await import("./mediaQueue");
    const store = await import("./reading");
    queue.resetMediaQueue();
    await store.loadReadingCatalog();

    const pending = store.requestIssue(DOCUMENT.key);
    await store.abortIssue(DOCUMENT.key);
    answer(FETCHING);
    await pending;
    await vi.advanceTimersByTimeAsync(2000);

    expect(store.issueStatus(DOCUMENT.key)).toBeUndefined();
    expect(queue.activeCount("r:")).toBe(0);
    expect(calls("get_issue_status").length).toBe(0);
  });

  // Install and read are the same job under the same key: a second click
  // while the first is running must not start a second transfer.
  it("does not start a second fetch for an issue already fetching", async () => {
    backend({
      list_publications: () => [],
      list_issues: () => [DISK_MAGAZINE],
      install_issue: () => FETCHING,
      get_issue_status: () => FETCHING,
    });
    const store = await import("./reading");
    await store.loadReadingCatalog();

    await store.installIssue(DISK_MAGAZINE.key);
    await store.installIssue(DISK_MAGAZINE.key);

    expect(calls("install_issue").length).toBe(1);
  });
});

describe("reading catalogue order", () => {
  const row = (over: Partial<Issue>): Issue => ({ ...DISK_MAGAZINE, ...over });
  const rows = [
    row({ key: "a", publication: "PC World", title: "PC World: Issue 10", sort_title: "PC World 10", year: 1984, release_date: "1984-12-01", size_bytes: 3 }),
    row({ key: "b", publication: "PC World", title: "PC World: Issue 9", sort_title: "PC World 9", year: 1984, release_date: "1984-11-01", size_bytes: 1 }),
    row({ key: "c", publication: "ACE", title: "ACE: Issue 01", sort_title: "ACE 01", year: 1987, release_date: "1987-10-01", size_bytes: 2 }),
    row({ key: "d", publication: "Softline", title: "Softline: Issue 5", sort_title: "Softline 5", year: null, release_date: null, size_bytes: 4 }),
  ];

  // Issue numbers compare numerically, and a series reads in publication
  // order, not title order.
  it("orders a publication by date and its titles numerically", async () => {
    const { sortIssues } = await import("./reading");
    expect(sortIssues(rows, "publication").map((r) => r.key)).toEqual(["c", "b", "a", "d"]);
    expect(sortIssues(rows, "title").map((r) => r.key)).toEqual(["c", "b", "a", "d"]);
  });

  // An unknown year sorts last in BOTH directions.
  it("keeps unknown years at the end either way", async () => {
    const { sortIssues } = await import("./reading");
    expect(sortIssues(rows, "year_desc").map((r) => r.key)).toEqual(["c", "a", "b", "d"]);
    expect(sortIssues(rows, "year_asc").map((r) => r.key)).toEqual(["b", "a", "c", "d"]);
  });

  it("labels sections by the active order", async () => {
    const { sectionOf } = await import("./reading");
    expect(sectionOf(rows[0], "publication")).toBe("PC World");
    expect(sectionOf(rows[0], "title")).toBe("P");
    expect(sectionOf(row({ sort_title: "3D Modeling Lab" }), "title")).toBe("#");
    expect(sectionOf(rows[3], "year_desc")).toBe("Unknown");
    expect(sectionOf(rows[0], "size")).toBe("");
  });

  // The series prefix goes only where the series is already in view, and
  // only in eXo's "Series: rest" form - "Big Blue Disk 004" keeps its name.
  it("drops the publication prefix only when the publication is in view", async () => {
    const { displayTitle } = await import("./reading");
    expect(displayTitle(rows[0], true)).toBe("Issue 10");
    expect(displayTitle(rows[0], false)).toBe("PC World: Issue 10");
    expect(displayTitle(DISK_MAGAZINE, true)).toBe("Big Blue Disk 004");
    expect(displayTitle(row({ publication: "interactive Entertainment CD", title: "Interactive Entertainment CD: Issue 17" }), true)).toBe("Issue 17");
  });
});

describe("reading language filter", () => {
  const row = (over: Partial<Issue>): Issue => ({ ...DISK_MAGAZINE, ...over });
  const english = row({ key: "en", publication_id: 1, publication: "ASM", title: "ASM Special", language: "EN" });
  const german = row({
    key: "de", publication_id: 2, publication: "ASM (DE)", title: "ASM 1986-03",
    source: "eXoDOS_GLP", inner_zip: "Content/eXoDOS_GLP_Addonpack_MagazinesGLP_1.0.zip", language: "DE",
  });
  const book = row({ key: "book", kind: "book", publication_id: 3, publication: "Sams", title: "DOS Power Tools", language: "EN" });
  const all = { kind: "all" as const, publicationId: null, query: "" };

  it("shows both languages under All and one under a chip", async () => {
    const { filterIssues } = await import("./reading");
    const keys = (rows: Issue[]) => rows.map((r) => r.key);
    expect(keys(filterIssues([english, german, book], { ...all, language: "all" }))).toEqual(["en", "de", "book"]);
    expect(keys(filterIssues([english, german, book], { ...all, language: "DE" }))).toEqual(["de"]);
    expect(keys(filterIssues([english, german, book], { ...all, language: "EN" }))).toEqual(["en", "book"]);
    // The chips combine with kind and search like any other filter.
    expect(keys(filterIssues([english, german, book], { ...all, kind: "magazine", language: "EN" }))).toEqual(["en"]);
    expect(keys(filterIssues([english, german, book], { ...all, language: "all", query: "1986-03" }))).toEqual(["de"]);
  });

  const publication = (over: Partial<Publication>): Publication => ({
    id: 0, kind: "magazine", name: "", issue_count: 1, first_year: null, last_year: null,
    cover_key: null, language: "EN", ...over,
  });
  const pubs = [
    publication({ id: 1, name: "PC Gamer" }),
    publication({ id: 2, name: "ASM (DE)", language: "DE" }),
    publication({ id: 3, name: "ACE" }),
    publication({ id: 4, kind: "book", name: "Sams" }),
    publication({ id: 5, name: "Power Play (DE)", language: "DE" }),
  ];

  // The German series get a group of their own only while both languages are
  // in view; a single-language view sorts them into their kind like any other.
  it("groups the (DE) publications under Deutsch only under All", async () => {
    const { publicationGroups } = await import("./reading");
    const shape = (groups: { label: string | null; rows: Publication[] }[]) =>
      groups.map((g) => [g.label, g.rows.map((p) => p.name)]);

    expect(shape(publicationGroups(pubs, "all", "all"))).toEqual([
      ["Magazines", ["ACE", "PC Gamer"]],
      ["Books", ["Sams"]],
      ["Deutsch", ["ASM (DE)", "Power Play (DE)"]],
    ]);
    expect(shape(publicationGroups(pubs, "magazine", "all"))).toEqual([
      [null, ["ACE", "PC Gamer"]],
      ["Deutsch", ["ASM (DE)", "Power Play (DE)"]],
    ]);
    expect(shape(publicationGroups(pubs, "all", "DE"))).toEqual([
      ["Magazines", ["ASM (DE)", "Power Play (DE)"]],
    ]);
    expect(shape(publicationGroups(pubs, "all", "EN"))).toEqual([
      ["Magazines", ["ACE", "PC Gamer"]],
      ["Books", ["Sams"]],
    ]);
  });
});
