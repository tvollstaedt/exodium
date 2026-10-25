import { $, $$, browser, expect } from "@wdio/globals";

// SQ5 is the canonical multi-language group (EN + DE + ES) and the example
// shortcode of the project docs, so it is the fixture here too. The search
// term is the EN title's tail: "Space Quest V" alone also hits SQ6 and the
// single-language ScummVM edition, whose sort_title puts it first.
describe("detail panel", () => {
  // Specs share one session (wdio.conf.ts), so leave the library as found.
  after(async () => {
    await $(".game-detail-close").click();
    await $(".search-clear").click();
  });

  it("opens the panel for the clicked card", async () => {
    await $('[data-testid="search-input"]').setValue("Roger Wilco The Next Mutation");
    // Wait for the search to APPLY: the previous grid's first card exists
    // the whole time, so "a card exists" proves nothing.
    await browser.waitUntil(
      async () => {
        const titles = await $$('[data-testid="game-card"] .game-card-title').map((t) => t.getText());
        return titles.length === 1 && /Roger Wilco/.test(titles[0]);
      },
      { timeoutMsg: "search did not narrow the grid to Space Quest V" },
    );
    const card = $('[data-testid="game-card"]');
    const title = await card.$(".game-card-title").getText();
    // The click handler sits on the art, and the art has no size until its
    // cover has loaded - wait for the image, not the card.
    await card.$(".game-card-thumb").waitForDisplayed();
    await card.$(".game-card-art").click();
    const panel = $('[data-testid="game-detail"]');
    await panel.waitForDisplayed();
    await expect(panel.$('[data-testid="game-detail-title"]')).toHaveText(title);
  });

  it("switches language variants inside the same panel", async () => {
    // `$$()` caches its result after the first await - re-query per poll.
    const chips = () => $$('[data-testid="variant-chip"]');
    await browser.waitUntil(async () => (await chips().length) >= 2, {
      timeoutMsg: "language chips did not load",
    });
    const selectedBefore = await $('[data-testid="variant-chip"].is-selected').getText();
    const other = (await chips().filter(async (c) => !(await c.getAttribute("class"))?.includes("is-selected")))[0];
    if (!other) { throw new Error("no unselected chip"); }
    await other.click();
    await browser.waitUntil(
      async () => (await $('[data-testid="variant-chip"].is-selected').getText()) !== selectedBefore,
      { timeoutMsg: "clicking a chip did not change the selection" },
    );
    // The panel is one component for every variant (§12): it must still be there.
    await expect($('[data-testid="game-detail"]')).toBeDisplayed();
  });

  it("toggles fullscreen on and off again", async () => {
    const panel = $('[data-testid="game-detail"]');
    const toggle = panel.$('[data-testid="game-detail-fullscreen"]');
    const windowWidth = await browser.execute(() => window.innerWidth);
    const before = (await panel.getSize()).width;
    expect(before).toBeLessThan(windowWidth);

    // Hover-revealed over the hero; moving onto it is what makes it clickable.
    await toggle.moveTo();
    await toggle.click();
    await browser.waitUntil(async () => (await panel.getAttribute("data-fullscreen")) === "true", {
      timeoutMsg: "the panel did not enter fullscreen",
    });
    expect((await panel.getSize()).width).toBe(windowWidth);
    // The title block stays in the same panel (nothing remounts).
    await expect(panel.$('[data-testid="game-detail-title"]')).toBeDisplayed();

    await toggle.click();
    await browser.waitUntil(async () => (await panel.getAttribute("data-fullscreen")) === "false", {
      timeoutMsg: "the panel did not leave fullscreen",
    });
    expect((await panel.getSize()).width).toBe(before);
  });

  it("drags the handle to widen the panel and double-clicks it to reset", async () => {
    const panel = $('[data-testid="game-detail"]');
    const handle = $('[data-testid="game-detail-resizer"]');
    const before = (await panel.getSize()).width;

    await browser.action("pointer")
      .move({ origin: handle })
      .down()
      .move({ origin: "pointer", x: -200, y: 0, duration: 100 })
      .up()
      .perform();
    await browser.waitUntil(async () => (await panel.getSize()).width > before + 100, {
      timeoutMsg: "dragging the handle did not widen the panel",
    });

    await handle.doubleClick();
    await browser.waitUntil(async () => (await panel.getSize()).width === before, {
      timeoutMsg: "double-clicking the handle did not reset the width",
    });
  });
});
