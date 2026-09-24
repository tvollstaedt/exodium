import { describe, it, expect, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { initStartupTaskEvents, startupTask } from "./startupTasks";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

describe("startup tasks", () => {
  it("shows the task from started until done", async () => {
    await initStartupTaskEvents();
    const handler = vi.mocked(listen).mock.calls[0][1] as (e: { payload: unknown }) => void;
    expect(startupTask()).toBeNull();
    handler({ payload: { task: "sparse", state: "started", freed_mb: 0 } });
    expect(startupTask()?.task).toBe("sparse");
    handler({ payload: { task: "sparse", state: "done", freed_mb: 64463 } });
    expect(startupTask()).toBeNull();
  });
});
