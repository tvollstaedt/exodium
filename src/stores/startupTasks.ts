import { createSignal } from "solid-js";
import { listen } from "@tauri-apps/api/event";

export interface StartupTask {
  task: "sparse";
  state: "started" | "done";
  freed_mb: number;
}

const [startupTask, setStartupTask] = createSignal<StartupTask | null>(null);
export { startupTask };

/** A one-time job the backend runs inside `init_download_manager` (the
 *  sparse-file reclaim, §21). Registered before the first invoke, or the
 *  `started` event lands on nobody. */
export async function initStartupTaskEvents() {
  await listen<StartupTask>("startup-task", (event) => {
    setStartupTask(event.payload.state === "done" ? null : event.payload);
  });
}
