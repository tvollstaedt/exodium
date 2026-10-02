import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import { SettingsDialog } from "./SettingsDialog";

const mockInvoke = vi.mocked(invoke);

async function flush() {
  for (let i = 0; i < 4; i++) {
    await new Promise((r) => setTimeout(r, 0));
  }
}

function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  return render(
    () => (
      <SettingsDialog
        open
        onOpenChange={() => {}}
        section="general"
        onSectionChange={() => {}}
        gameFolderPath="/games"
        onChangeDataDir={() => {}}
        layoutSkipped={false}
        migrating={false}
        onMergeLayout={() => {}}
        onFactoryReset={() => {}}
        resetError=""
        onWentOnline={() => {}}
      />
    ),
    host,
  );
}

const previewSound = () => document.getElementById("preview-sound") as HTMLInputElement;

/** The speaker button on the preview and this switch are one preference
 *  (`preview_muted`), stored as "muted", shown as "sound". */
describe("SettingsDialog preview sound", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("shows the stored mute preference inverted and writes it back", async () => {
    mockInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "get_config" && (args as { key: string }).key === "preview_muted") { return "1"; }
      return null;
    });
    const dispose = mount();
    await flush();

    expect(previewSound().checked).toBe(false);

    previewSound().click();
    await flush();

    expect(mockInvoke).toHaveBeenCalledWith("set_config", { key: "preview_muted", value: "0" });
    expect(previewSound().checked).toBe(true);
    dispose();
  });
});
