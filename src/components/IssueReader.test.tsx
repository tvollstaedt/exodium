import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import type { Issue } from "../api/tauri";

const mockInvoke = vi.mocked(invoke);

const READY = {
  phase: "ready", progress: 1, total_bytes: 57_000_000,
  path: "/data/content/magazinecache/abc.jpg", error: null,
};

/** A cover scan: kind "image", so the viewer needs no pdf.js. */
const SCAN: Issue = {
  id: 1,
  key: "mag:eXo/Magazines/PCWorld#022",
  publication_id: 1,
  publication: "PC World",
  kind: "magazine",
  title: "PC World: Issue 22",
  sort_title: null,
  year: 1984,
  release_date: null,
  publisher: null,
  developer: null,
  notes: null,
  source: "eXoMedia",
  zip_file: "Content/DOSMagazines.zip",
  inner_zip: null,
  entry_path: "eXo/Magazines/PCWorld/022.jpg",
  entry_kind: "image",
  size_bytes: 57_000_000,
  cover_key: null,
  runnable: false,
  launch_dir: null,
  issue_dir: null,
  launch_bat: null,
  command_line: null,
  substitutions: null,
  language: "EN",
  extras_count: 0,
  favorited: false,
  installed: false,
  last_page: null,
  last_opened: null,
};

const OTHER: Issue = { ...SCAN, id: 2, key: "mag:eXo/Magazines/ACE#001", title: "ACE: Issue 1" };

function backend(handlers: Record<string, (args: any) => any>) {
  mockInvoke.mockImplementation(async (cmd: string, args: any) => {
    const h = handlers[cmd];
    return h ? h(args ?? {}) : null;
  });
}

const calls = (cmd: string) => mockInvoke.mock.calls.filter((c) => c[0] === cmd);

/** The viewer portals into the body, so assertions look there. */
function mount(node: () => any) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const dispose = render(node, host);
  return () => { dispose(); host.remove(); };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("IssueReader", () => {
  let dispose: (() => void) | null = null;

  beforeEach(() => {
    mockInvoke.mockReset();
    vi.resetModules();
  });
  afterEach(() => {
    dispose?.();
    dispose = null;
    document.body.innerHTML = "";
  });

  // The fetch request writes the very state the panel renders from, so a
  // tracked call re-triggers itself: offline it answers instantly and the
  // effect used to recurse until the stack blew.
  it("renders the offline panel without looping", async () => {
    backend({ get_config: () => "offline" });
    const network = await import("../stores/network");
    await network.loadNetworkMode();
    const { IssueReader } = await import("./IssueReader");

    dispose = mount(() => <IssueReader issue={SCAN} onClose={() => {}} />);
    await flush();

    const panel = document.body.querySelector('[data-testid="issue-fetch"]');
    expect(panel?.getAttribute("data-phase")).toBe("none");
    expect(panel?.textContent).toContain("Offline");
    expect(calls("open_issue").length).toBe(0);
  });

  // The reader is scoped to one issue: another issue's status has no business
  // re-running anything here.
  it("keeps the open document mounted through an unrelated status change", async () => {
    backend({
      list_publications: () => [],
      list_issues: () => [SCAN, OTHER],
      open_issue: (args: any) =>
        (args.key === SCAN.key
          ? READY
          : { phase: "error", progress: 0, total_bytes: 0, path: null, error: "nope" }),
    });
    const store = await import("../stores/reading");
    const { IssueReader } = await import("./IssueReader");

    dispose = mount(() => <IssueReader issue={SCAN} onClose={() => {}} />);
    await flush();

    const image = document.body.querySelector(".document-viewer-image img");
    expect(image).not.toBeNull();

    await store.requestIssue(OTHER.key);
    await flush();

    expect(document.body.querySelector(".document-viewer-image img")).toBe(image);
  });
});
