import { $, $$, browser, expect } from "@wdio/globals";

// A playlist made from a card's menu, then "Download playlist" priced against the
// seeded catalogue - which is offline, so the dialog must refuse to start.
// Leaves no playlist behind: the library spec expects an empty My Library.
describe("playlists", () => {
  it("creates a playlist from a card's menu", async () => {
    await $('[data-testid="search-input"]').setValue("Commander Keen 4");
    let index = -1;
    await browser.waitUntil(
      async () => {
        const titles = await $$('[data-testid="game-card"] .game-card-title').map((t) => t.getText());
        index = titles.findIndex((t) => /^Commander Keen 4/.test(t));
        return titles.length < 20 && index >= 0;
      },
      { timeoutMsg: "search did not narrow the grid to Commander Keen 4" },
    );
    const card = await $$('[data-testid="game-card"]')[index];
    await browser.execute((el: HTMLElement) => {
      const r = el.getBoundingClientRect();
      el.dispatchEvent(new MouseEvent("contextmenu", {
        bubbles: true, cancelable: true, clientX: r.left + 20, clientY: r.top + 20,
      }));
    }, card as unknown as HTMLElement);
    await $('[data-testid="menu-add-to-playlist"]').click();
    await $('[data-testid="playlist-menu-new"]').click();
    await $('[data-testid="playlist-name-input"]').setValue("E2E list");
    await $('[data-testid="playlist-name-save"]').click();
    await $(".search-clear").click();

    await $('[data-testid="tab-library"]').click();
    await $('[data-testid="shelf-menu"]').waitForExist({ timeoutMsg: "the playlist shelf did not appear" });
  });

  it("prices Download playlist and refuses to start offline", async () => {
    await $('[data-testid="shelf-menu"]').click();
    await $('[data-testid="playlist-download-all"]').click();
    const facts = $('[data-testid="download-all-facts"]');
    await browser.waitUntil(async () => /^1 game to download · \d/.test(await facts.getText()), {
      timeoutMsg: "the plan did not load",
    });
    await expect($('[data-testid="download-all-start"]')).toBeDisabled();
    expect(await $('[data-testid="download-all-dialog"]').getText()).toContain("Offline mode");
    await $('[data-testid="download-all-cancel"]').click();
    await $('[data-testid="download-all-dialog"]').waitForExist({ reverse: true });
  });

  it("deletes the playlist again", async () => {
    await $('[data-testid="shelf-menu"]').click();
    await $('[data-testid="playlist-delete"]').click();
    await $('[data-testid="confirm-ok"]').click();
    await $(".lib-empty").waitForDisplayed();
    await $('[data-testid="tab-browse"]').click();
    // The toasts this spec raised sit over the bottom-right corner, where the
    // next spec's dialog has its Close button.
    await $('[data-testid="toast"]').waitForExist({ reverse: true, timeout: 15_000 });
  });
});
