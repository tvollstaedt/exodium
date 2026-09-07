import { $, $$, browser, expect } from "@wdio/globals";

// The seeded catalogue is the bundled one, so counts are real and stable.
describe("library", () => {
  it("starts into the Browse grid with cards", async () => {
    await expect(browser).toHaveTitle("Exodium");
    await $('[data-testid="browse-grid"]').waitForExist();
    await browser.waitUntil(async () => (await $$('[data-testid="game-card"]').length) > 0, {
      timeoutMsg: "no game card rendered",
    });
    await expect($('[data-testid="tab-browse"]')).toHaveElementClass("active");
  });

  it("filters the grid by title search", async () => {
    const search = $('[data-testid="search-input"]');
    await search.setValue("Space Quest V");
    await browser.waitUntil(
      async () => {
        const titles = await $$('[data-testid="game-card"] .game-card-title').map((t) => t.getText());
        return titles.length > 0 && titles.length < 20 && titles.every((t) => /space quest/i.test(t));
      },
      { timeoutMsg: "search did not narrow the grid to Space Quest" },
    );
    await $(".search-clear").click();
    await browser.waitUntil(async () => (await $$('[data-testid="game-card"]').length) >= 20, {
      timeoutMsg: "clearing the search did not restore the grid",
    });
  });

  it("shows the empty library and returns to Browse", async () => {
    await $('[data-testid="tab-library"]').click();
    await expect($(".lib-empty")).toBeDisplayed();
    await $('[data-testid="tab-browse"]').click();
    await expect($('[data-testid="browse-grid"]')).toBeDisplayed();
  });
});
