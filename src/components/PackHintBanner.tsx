import { createSignal, Show } from "solid-js";
import { getConfig, setConfig } from "../api/tauri";
import { startContentPackInstall, activeJobs, packsByCollection } from "../stores/contentPacks";
import { isOffline } from "../stores/network";
import { posterDirForCollection, thumbnailDirsLoaded } from "../stores/thumbnails";
import { formatBytes } from "../util";
import { Button } from "./Button";

/** Config key holding the collections whose hint the user has dismissed. */
const DISMISSED_KEY = "pack_hint_dismissed";

interface Props {
  /** The collection currently being browsed. */
  collection: string;
}

/** One-time nudge above the Browse grid that a poster pack would sharpen
 *  this collection's covers (§10). Triggered by what the grid renders (Tier
 *  0), read from the store so it lands in the same frame; dismissed per
 *  collection for both answers. */
export function PackHintBanner(props: Props) {
  // null until the stored list has arrived. Treating "not loaded yet" as "not
  // dismissed" made the banner flash on every start before the config landed.
  const [dismissed, setDismissed] = createSignal<string[] | null>(null);

  getConfig(DISMISSED_KEY)
    .then((v) => setDismissed(v ? v.split(",").filter(Boolean) : []))
    // An unreadable config must not mean "nag on every start": stay quiet.
    .catch(() => setDismissed(["*"]));

  /** Already downloading it? Then the hint has done its job. */
  const running = () =>
    Object.entries(activeJobs()).some(
      ([key, job]) => job && !job.finished && key.startsWith(`${props.collection}:`),
    );

  const pack = () => {
    const collection = props.collection;
    if (!collection || isOffline() || running()) { return null; }
    const seen = dismissed();
    if (!seen || seen.includes(collection)) { return null; }
    // Only when the grid is on the bundled low-res tier. Wait for the resolve:
    // the startup state is an empty map, which would read as "no posters".
    if (!thumbnailDirsLoaded() || posterDirForCollection(collection)) { return null; }
    // Only the box-art tier: it is the one that changes what the user is
    // looking at. Gallery art and manuals live in the metadata pack, which
    // runs to 24 GB (GLP) and belongs in Settings, not in a drive-by hint.
    const packs = packsByCollection()[collection] ?? [];
    return packs.find((p) => p.id === "posters" && p.available && !p.installed) ?? null;
  };

  const remember = async (collection: string) => {
    const next = [...new Set([...(dismissed() ?? []), collection])];
    setDismissed(next);
    await setConfig(DISMISSED_KEY, next.join(",")).catch(() => {});
  };

  const install = async () => {
    const p = pack();
    const collection = props.collection;
    if (!p) { return; }
    // Remember first: a failed start is still an answered question.
    await remember(collection);
    startContentPackInstall(collection, p.id, p.display_name).catch((e) =>
      console.error("Failed to start content pack install:", e),
    );
  };

  return (
    <Show when={pack()}>
      <div class="pack-hint">
        <div class="pack-hint-text">
          <div class="pack-hint-title">Better covers available</div>
          <div class="pack-hint-desc">
            {pack()!.display_name} for this collection is an optional{" "}
            {formatBytes(pack()!.size_bytes)} download. You can also manage it later
            in Settings.
          </div>
        </div>
        <Button variant="secondary" class="pack-hint-action" onClick={install}>Download</Button>
        <button
          class="pack-hint-dismiss"
          title="Not now"
          onClick={() => remember(props.collection)}
        >✕</button>
      </div>
    </Show>
  );
}
