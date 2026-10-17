import { createSignal } from "solid-js";
import { setAppleiigsOption } from "../api/tauri";

/** Bumped whenever an Apple IIGS game's stored choice changes, so the panel
 *  re-reads it no matter which surface wrote it (chips or settings dialog). */
const [iigsRevision, setIigsRevision] = createSignal(0);
export { iigsRevision };

export async function chooseIigsOption(id: number, option: string | null): Promise<void> {
  await setAppleiigsOption(id, option);
  setIigsRevision((n) => n + 1);
}
