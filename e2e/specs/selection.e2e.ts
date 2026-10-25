import { $, $$, browser, expect } from "@wdio/globals";

// Multi-select: two cards picked in Select mode become a new playlist, one is
// removed again from its shelf. Leaves no playlist and no toast behind - the
// specs share one session.
describe("multi-select", () => {
  const shelf = () => $('[data-select-zone^="shelf:pl-"]');
  const shelfCards = () => shelf().$$('[data-testid="game-card"]');

  it("selects two games and makes them a playlist", async () => {
    await $('[data-testid="search-input"]').setValue("Commander Keen");
    await browser.waitUntil(async () => (await $$('[data-testid="game-card"]').length) >= 2, {
      timeoutMsg: "search found fewer than two Commander Keen cards",
    });
    await $('[data-testid="select-mode"]').click();
    for (const i of [0, 1]) {
      const card = $$('[data-testid="game-card"]')[i];
      await card.$(".game-card-thumb").waitForDisplayed();
      await card.$(".game-card-art").click();
    }
    await expect($('[data-testid="selection-count"]')).toHaveText("2 games selected");
    await expect($('[data-testid="game-detail"]')).not.toBeDisplayed();

    // Escape in the dialog closes the dialog, not the selection.
    await $('[data-testid="selection-add"]').click();
    await $('[data-testid="selection-new-playlist"]').click();
    await $('[data-testid="playlist-name-input"]').waitForDisplayed();
    await browser.keys("Escape");
    await $('[data-testid="playlist-name-input"]').waitForExist({ reverse: true });
    await expect($('[data-testid="selection-count"]')).toHaveText("2 games selected");

    await $('[data-testid="selection-add"]').click();
    await $('[data-testid="selection-new-playlist"]').click();
    await $('[data-testid="playlist-name-input"]').setValue("E2E selection");
    await $('[data-testid="playlist-name-save"]').click();
    await $('[data-testid="playlist-name-input"]').waitForExist({ reverse: true });
    await browser.keys("Escape");
    await $('[data-testid="selection-bar"]').waitForExist({ reverse: true });
    await $(".search-clear").click();
  });

  it("removes one of them from its shelf", async () => {
    await $('[data-testid="tab-library"]').click();
    await browser.waitUntil(async () => (await shelfCards().length) === 2, { timeoutMsg: "the new shelf does not hold two games" });
    await $('[data-testid="select-mode"]').click();
    const card = shelfCards()[0];
    await card.$(".game-card-thumb").waitForDisplayed();
    await card.$(".game-card-art").click();
    await $('[data-testid="selection-remove"]').click();
    await browser.waitUntil(async () => (await shelfCards().length) === 1, { timeoutMsg: "the game was not removed" });
    await browser.keys("Escape");
  });

  it("deletes the playlist again", async () => {
    await $('[data-testid="shelf-menu"]').click();
    await $('[data-testid="playlist-delete"]').click();
    await $('[data-testid="confirm-ok"]').click();
    // Not ".lib-empty": the startup scan may confirm an extracted game the
    // test data dir still holds, and that shelf turns up later.
    await shelf().waitForExist({ reverse: true });
    await $('[data-testid="tab-browse"]').click();
    await $('[data-testid="toast"]').waitForExist({ reverse: true, timeout: 15_000 });
  });
});
