import { For, Show, createSignal, onCleanup } from "solid-js";
import {
  extraJobKey, getIssueStatus, openGameExtra, removeGameExtra, type IigsExtra, type ReadingStatus,
} from "../api/tauri";
import { DocumentViewer, type DocumentKind } from "./DocumentViewer";
import { formatBytes } from "../util";
import { showToast } from "../stores/toasts";
import { isOffline } from "../stores/network";

const kindOf = (name: string): DocumentKind => {
  if (/\.(jpe?g|png|gif)$/i.test(name)) { return "image"; }
  if (/\.txt$/i.test(name)) { return "txt"; }
  if (/\.html?$/i.test(name)) { return "html"; }
  return "pdf";
};

/** "Jack_Nicklaus_Courses_of_1989-Course_Sheet.pdf" -> "Jack Nicklaus
 *  Courses of 1989 - Course Sheet": eXo names the files, not the documents. */
export function extraLabel(name: string): string {
  return name
    .replace(/\.[^./]+$/, "")
    .split("/")
    .map((part) => part.replace(/_/g, " ").replace(/\s*-\s*/g, " - ").trim())
    .join(" / ");
}

interface Props {
  gameId: number;
  extras: IigsExtra[];
  /** A fetch finished or a file was removed: the caller re-reads `cached`. */
  onChanged: () => void;
}

/** Reference cards, maps and manuals eXo keeps per Apple IIGS game. Each is
 *  read out of the torrent on the first click and kept until removed (§23). */
export function IigsExtras(props: Props) {
  const [progress, setProgress] = createSignal<Record<string, number>>({});
  const [viewing, setViewing] = createSignal<{ name: string; path: string } | null>(null);
  const timers = new Set<ReturnType<typeof setTimeout>>();
  onCleanup(() => timers.forEach(clearTimeout));

  const setPct = (name: string, value: number | null) =>
    setProgress((p) => {
      const next = { ...p };
      if (value == null) { delete next[name]; } else { next[name] = value; }
      return next;
    });

  const settle = (name: string, status: ReadingStatus | null, id: number) => {
    if (status?.phase === "ready" && status.path) {
      setPct(name, null);
      props.onChanged();
      if (props.gameId === id) { setViewing({ name, path: status.path }); }
      return;
    }
    if (!status || status.phase !== "fetching") {
      setPct(name, null);
      const offline = status?.phase === "none";
      showToast(offline ? "Go online to fetch this document" : `Couldn't fetch ${extraLabel(name)}`,
        "error", status?.error && !offline ? { detail: status.error } : undefined);
      return;
    }
    setPct(name, status.progress);
    const timer = setTimeout(() => {
      timers.delete(timer);
      getIssueStatus(extraJobKey(id, name))
        .then((s) => settle(name, s, id))
        .catch((e) => settle(name, { ...status, phase: "error", error: String(e) }, id));
    }, 1000);
    timers.add(timer);
  };

  const open = async (extra: IigsExtra) => {
    if (progress()[extra.name] != null) { return; }
    if (!extra.cached && !extra.on_disk && isOffline()) {
      showToast("Go online to fetch this document", "error");
      return;
    }
    const id = props.gameId;
    setPct(extra.name, 0);
    try {
      settle(extra.name, await openGameExtra(id, extra.name), id);
    } catch (e) {
      setPct(extra.name, null);
      showToast(`Couldn't fetch ${extraLabel(extra.name)}`, "error", { detail: String(e) });
    }
  };

  const remove = async (extra: IigsExtra) => {
    try {
      await removeGameExtra(props.gameId, extra.name);
      props.onChanged();
    } catch (e) {
      showToast(`Couldn't remove ${extraLabel(extra.name)}`, "error", { detail: String(e) });
    }
  };

  return (
    <div class="game-detail-extras" data-testid="iigs-extras">
      <div class="game-detail-section-label">
        Extras
        <span class="section-count">{props.extras.length}</span>
      </div>
      <div class="game-detail-article-list">
        <For each={props.extras}>
          {(extra) => (
            <div class="game-detail-extra">
              <button
                class="game-detail-article"
                title={extra.name}
                onClick={() => { void open(extra); }}
              >
                <span class="game-detail-article-issue">{extraLabel(extra.name)}</span>
                <span class="game-detail-article-page">
                  <Show
                    when={progress()[extra.name] != null}
                    fallback={extra.cached || extra.on_disk ? "✓" : formatBytes(extra.size)}
                  >
                    {Math.round((progress()[extra.name] ?? 0) * 100)}%
                  </Show>
                </span>
              </button>
              <Show when={extra.cached}>
                <button
                  class="game-detail-extra-remove"
                  title="Remove the downloaded copy"
                  aria-label={`Remove ${extraLabel(extra.name)}`}
                  onClick={() => { void remove(extra); }}
                >✕</button>
              </Show>
            </div>
          )}
        </For>
      </div>
      <DocumentViewer
        open={viewing() != null}
        title={viewing() ? extraLabel(viewing()!.name) : ""}
        subtitle="Extra"
        source={viewing() ? { kind: kindOf(viewing()!.name), path: viewing()!.path } : null}
        onClose={() => setViewing(null)}
        testId="extra-viewer"
      />
    </div>
  );
}
