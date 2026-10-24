import { $, expect } from "@wdio/globals";

// Kids mode: turned on, locked, a wrong PIN refused, the right one opens it,
// turned off again. Removes the "Kids" playlist it created - the specs share
// one session.
describe("kids mode", () => {
  const openKidsSettings = async () => {
    await $('[data-testid="open-settings"]').click();
    await $('[data-testid="settings-tab-kids"]').click();
    await $('[data-testid="kids-settings"]').waitForDisplayed();
  };

  it("turns on with a PIN and a new Kids playlist", async () => {
    await openKidsSettings();
    await $('[data-testid="kids-pin-new"]').setValue("1234");
    await $('[data-testid="kids-pin-repeat"]').setValue("1234");
    await $('[data-testid="kids-enable"]').click();
    await $('[data-testid="kids-disable"]').waitForExist({ timeoutMsg: "kids mode did not turn on" });
    await $('[data-testid="settings-close"]').click();
    await $('[data-testid="toast"]').waitForExist({ reverse: true, timeout: 15_000 });
  });

  it("locks to the kids screen and nothing else", async () => {
    await $('[data-testid="kids-lock"]').click();
    await $('[data-testid="kids-screen"]').waitForDisplayed();
    await expect($('[data-testid="kids-empty"]')).toBeDisplayed();
    await expect($('[data-testid="tab-browse"]')).not.toExist();
    await expect($('[data-testid="search-input"]')).not.toExist();
    await expect($('[data-testid="open-settings"]')).not.toExist();
  });

  it("refuses a wrong PIN and opens with the right one", async () => {
    await $('[data-testid="kids-unlock"]').click();
    await $('[data-testid="kids-pin-input"]').setValue("0000");
    await $('[data-testid="kids-pin-submit"]').click();
    await expect($('[data-testid="kids-pin-error"]')).toHaveText("Wrong PIN");
    await expect($('[data-testid="kids-screen"]')).toBeDisplayed();

    await $('[data-testid="kids-pin-input"]').setValue("1234");
    await $('[data-testid="kids-pin-submit"]').click();
    await $('[data-testid="kids-screen"]').waitForExist({ reverse: true });
    await expect($('[data-testid="tab-browse"]')).toBeDisplayed();
  });

  it("turns off with the PIN and leaves no playlist behind", async () => {
    await openKidsSettings();
    await $('[data-testid="kids-pin-off"]').setValue("1234");
    await $('[data-testid="kids-disable"]').click();
    await $('[data-testid="kids-enable"]').waitForExist({ timeoutMsg: "kids mode did not turn off" });
    await $('[data-testid="settings-close"]').click();
    await expect($('[data-testid="kids-lock"]')).not.toExist();

    await $('[data-testid="tab-library"]').click();
    await $('[data-testid="shelf-menu"]').click();
    await $('[data-testid="playlist-delete"]').click();
    await $('[data-testid="confirm-ok"]').click();
    await $('[data-testid="shelf-menu"]').waitForExist({ reverse: true });
    await $('[data-testid="tab-browse"]').click();
    await $('[data-testid="toast"]').waitForExist({ reverse: true, timeout: 15_000 });
  });
});
