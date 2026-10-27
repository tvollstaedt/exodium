import { $, expect } from "@wdio/globals";

// Kids mode: turned on (which locks at once), a wrong PIN refused, the right
// one opens it, locked again from the top bar and from Settings, turned off.
// Removes the "Kids" playlist it created - the specs share one session.
describe("kids mode", () => {
  const openKidsSettings = async () => {
    await $('[data-testid="open-settings"]').click();
    await $('[data-testid="settings-tab-kids"]').click();
    await $('[data-testid="kids-settings"]').waitForDisplayed();
  };

  const unlock = async () => {
    await $('[data-testid="kids-unlock"]').click();
    await $('[data-testid="kids-pin-input"]').setValue("1234");
    await $('[data-testid="kids-pin-submit"]').click();
    await $('[data-testid="kids-screen"]').waitForExist({ reverse: true });
  };

  it("turns on with a PIN typed twice and a new Kids playlist, and locks right away", async () => {
    await openKidsSettings();
    await expect($('[data-testid="kids-status"]')).toHaveText("Off");
    await $('[data-testid="kids-turn-on"]').click();
    await $('[data-testid="kids-pin-new"]').setValue("1234");
    await $('[data-testid="kids-pin-repeat"]').setValue("1234");
    await $('[data-testid="kids-enable"]').click();
    await $('[data-testid="kids-screen"]').waitForDisplayed({ timeoutMsg: "turning kids mode on did not lock" });
  });

  it("shows the kids screen and nothing else", async () => {
    await expect($('[data-testid="kids-empty"]')).toBeDisplayed();
    await expect($('[data-testid="tab-browse"]')).not.toExist();
    await expect($('[data-testid="search-input"]')).not.toExist();
    await expect($('[data-testid="open-settings"]')).not.toExist();
    await expect($('[data-testid="settings-dialog"]')).not.toExist();
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
    // Settings closed before it locked, so the unlock does not bring it back.
    await expect($('[data-testid="settings-dialog"]')).not.toExist();
  });

  it("locks again from the top bar and from Settings", async () => {
    await $('[data-testid="kids-lock"]').click();
    await $('[data-testid="kids-screen"]').waitForDisplayed();
    await unlock();

    await openKidsSettings();
    await expect($('[data-testid="kids-status"]')).toHaveText("On");
    await $('[data-testid="kids-lock-now"]').click();
    await $('[data-testid="kids-screen"]').waitForDisplayed();
    await unlock();
    await expect($('[data-testid="settings-dialog"]')).not.toExist();
  });

  it("turns off with the PIN and leaves no playlist behind", async () => {
    await openKidsSettings();
    await $('[data-testid="kids-turn-off"]').click();
    await $('[data-testid="kids-pin-current"]').setValue("1234");
    await $('[data-testid="kids-disable"]').click();
    await $('[data-testid="kids-turn-on"]').waitForExist({ timeoutMsg: "kids mode did not turn off" });
    await expect($('[data-testid="kids-status"]')).toHaveText("Off");
    await $('[data-testid="settings-close"]').click();
    await expect($('[data-testid="kids-lock"]')).not.toExist();

    // The playlist's own menu leads back to Settings with it chosen.
    await $('[data-testid="tab-library"]').click();
    await $('[data-testid="shelf-menu"]').click();
    await $('[data-testid="playlist-use-for-kids"]').click();
    await $('[data-testid="kids-settings"]').waitForDisplayed();
    await expect($('[data-testid="kids-playlist"] option:checked')).toHaveText("Kids");
    await $('[data-testid="settings-close"]').click();

    await $('[data-testid="shelf-menu"]').click();
    await $('[data-testid="playlist-delete"]').click();
    await $('[data-testid="confirm-ok"]').click();
    await $('[data-testid="shelf-menu"]').waitForExist({ reverse: true });
    await $('[data-testid="tab-browse"]').click();
    await $('[data-testid="toast"]').waitForExist({ reverse: true, timeout: 15_000 });
  });
});
