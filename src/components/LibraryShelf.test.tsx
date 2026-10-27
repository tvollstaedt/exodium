import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { createSignal, For } from "solid-js";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import type { GameRef } from "../api/tauri";

const mockInvoke = vi.mocked(invoke);
const KEYS = ["recent", "installed", "pl-5"];
const ref = (id: number): GameRef =>
  ({ id, title: `Game ${id}`, torrent_source: "eXoDOS", thumbnail_key: null, installed: true, in_library: true, favorited: false });

/** Backend with `collapsed_shelves` stored as `stored`; returns the writes. */
function backend(stored: string | null) {
  const writes: string[] = [];
  mockInvoke.mockImplementation((async (cmd: string, args?: { key?: string; value?: string }) => {
    if (cmd === "get_config") { return args?.key === "collapsed_shelves" ? stored : null; }
    if (cmd === "set_config" && args?.key === "collapsed_shelves") { writes.push(args.value!); }
    return cmd === "get_playlists" ? [] : null;
  }) as typeof invoke);
  return writes;
}

async function mount(searching: () => boolean = () => false) {
  const { LibraryShelf } = await import("./LibraryShelf");
  const host = document.createElement("div");
  document.body.appendChild(host);
  const dispose = render(() => (
    <For each={KEYS}>
      {(key) => (
        <LibraryShelf
          shelf={key}
          title={key}
          count={1}
          allShelves={KEYS}
          searching={searching()}
          actions={key === "pl-5" ? <button class="shelf-menu-btn">⋯</button> : undefined}
        >
          <div class="shelf-body" />
        </LibraryShelf>
      )}
    </For>
  ), host);
  for (let i = 0; i < 10; i++) { await Promise.resolve(); }
  const section = (key: string) => host.querySelector<HTMLElement>(`[data-shelf-key="${key}"]`)!;
  const toggle = (key: string) => section(key).querySelector<HTMLButtonElement>("[data-testid=shelf-toggle]")!;
  const collapsed = (key: string) => section(key).dataset.collapsed === "true";
  const open = () => KEYS.filter((k) => section(k).querySelector(".shelf-body"));
  return { dispose, section, toggle, collapsed, open };
}

describe("LibraryShelf", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    vi.resetModules();
  });
  afterEach(() => { document.body.innerHTML = ""; });

  it("folds from its header, stores the folded keys and leaves the menu alone", async () => {
    const writes = backend("installed");
    const s = await mount();
    expect(s.collapsed("installed")).toBe(true);
    expect(s.toggle("installed").getAttribute("aria-expanded")).toBe("false");
    expect(s.open()).toEqual(["recent", "pl-5"]);

    s.toggle("recent").click();
    expect(s.open()).toEqual(["pl-5"]);
    s.toggle("installed").click();
    expect(s.open()).toEqual(["installed", "pl-5"]);
    expect(writes).toEqual(["installed,recent", "recent"]);

    s.section("pl-5").querySelector<HTMLButtonElement>(".shelf-menu-btn")!.click();
    expect(s.collapsed("pl-5")).toBe(false);
    expect(writes).toHaveLength(2);
    s.dispose();
  });

  it("Alt-click takes every shelf to the clicked one's new state", async () => {
    const writes = backend("installed");
    const s = await mount();
    s.toggle("recent").dispatchEvent(new MouseEvent("click", { bubbles: true, altKey: true }));
    expect(s.open()).toEqual([]);
    expect(writes[writes.length - 1].split(",").sort()).toEqual([...KEYS].sort());

    s.toggle("installed").dispatchEvent(new MouseEvent("click", { bubbles: true, altKey: true }));
    expect(s.open()).toEqual(KEYS);
    expect(writes[writes.length - 1]).toBe("");
    s.dispose();
  });

  it("opens every shelf while searching and restores the stored state after", async () => {
    const writes = backend("installed,pl-5");
    const shelves = await import("../stores/shelves");
    const [searching, setSearching] = createSignal(true);
    const s = await mount(searching);
    expect(s.open()).toEqual(KEYS);

    s.toggle("recent").click();
    expect(s.open()).toEqual(["installed", "pl-5"]);
    expect(writes).toEqual([]);

    // Library ends the search session when the query clears.
    setSearching(false);
    shelves.endShelfSearch();
    expect(s.open()).toEqual(["recent"]);

    setSearching(true);
    expect(s.open()).toEqual(KEYS);
    s.dispose();
  });

  it("drops a selection its shelf hides, and only that one", async () => {
    backend(null);
    const selection = await import("../stores/selection");
    const s = await mount();

    selection.selectRefs("shelf:installed", [ref(1), ref(2)]);
    s.toggle("recent").click();
    expect(selection.selectionCount()).toBe(2);

    s.toggle("installed").click();
    expect(selection.selectionCount()).toBe(0);
    s.dispose();
  });

  it("renders no games and writes nothing before the stored state arrives", async () => {
    let answer: (v: string | null) => void = () => {};
    const writes: string[] = [];
    mockInvoke.mockImplementation((async (cmd: string, args?: { value?: string }) => {
      if (cmd === "get_config") { return new Promise<string | null>((r) => { answer = r; }); }
      if (cmd === "set_config") { writes.push(args!.value!); }
      return null;
    }) as typeof invoke);
    const s = await mount();
    expect(s.open()).toEqual([]);
    s.toggle("recent").click();
    expect(writes).toEqual([]);

    answer("installed");
    for (let i = 0; i < 10; i++) { await Promise.resolve(); }
    expect(s.open()).toEqual(["recent", "pl-5"]);
    s.dispose();
  });

  it("forgets a deleted playlist, whose id the next one may get", async () => {
    const writes = backend("installed,pl-5");
    const shelves = await import("../stores/shelves");
    const playlists = await import("../stores/playlists");
    shelves.ensureShelvesLoaded();
    await vi.waitFor(() => expect(shelves.shelvesLoaded()).toBe(true));

    await playlists.deletePlaylist(5);
    expect(writes).toEqual(["installed"]);
    expect(shelves.isShelfCollapsed("pl-5", false)).toBe(false);
  });
});
