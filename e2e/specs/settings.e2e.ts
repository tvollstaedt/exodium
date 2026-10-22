import { $, browser, expect } from "@wdio/globals";

// The manifest as the user meets it: a platform-mapped pack resolves to this
// OS's source or vanishes, and one without a published source reads "Coming
// soon" (§10). Before a release that state means a placeholder slipped through.
describe("settings: content packs", () => {
  after(async () => {
    await $('[data-testid="settings-close"]').click();
    await $('[data-testid="settings-dialog"]').waitForDisplayed({ reverse: true });
  });

  it("opens the content packs section", async () => {
    await $('[data-testid="open-settings"]').click();
    await $('[data-testid="settings-dialog"]').waitForDisplayed();
    await $('[data-testid="settings-tab-packs"]').click();
    await $('[data-testid="content-pack"]').waitForExist({ timeoutMsg: "no content pack rows rendered" });
  });

  it("lists no pack as coming soon", async () => {
    const soon = await browser.execute(() =>
      Array.from(document.querySelectorAll('[data-testid="content-pack"]'))
        .filter((row) => row.textContent?.includes("Coming soon"))
        .map((row) => row.getAttribute("data-pack")),
    );
    expect(soon).toEqual([]);
  });

  it("resolves the GSplus pack to this platform's build", async () => {
    const row = $('[data-testid="content-pack"][data-pack="eXoAppleIIGS:gsplus"]');
    // Windows runs eXo's own builds and has no manifest entry (§23).
    if (process.platform === "win32") {
      await expect(row).not.toExist();
      return;
    }
    await row.scrollIntoView();
    const text = await row.getText();
    expect(text).toContain("GSplus 1.38");
    expect(text).not.toMatch(/· 0 B\b/);
  });
});
