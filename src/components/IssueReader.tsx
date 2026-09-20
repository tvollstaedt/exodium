import { createEffect, createMemo, createSignal, on, Show } from "solid-js";
import { type Issue, type ReadingStatus } from "../api/tauri";
import {
  issueStatus, requestIssue, abortIssue, rememberPage, removeIssue, isOnDisk, kindLabel,
  isFetchInFlight, PHASE_QUEUED,
} from "../stores/reading";
import { isOffline } from "../stores/network";
import { MEDIA_SOURCE } from "../stores/thumbnails";
import { formatBytes } from "../util";
import { createKeyedCover } from "./cover";
import { AutoProgress } from "./ProgressBar";
import { ConfirmDialog } from "./ConfirmDialog";
import { DocumentViewer } from "./DocumentViewer";

interface IssueReaderProps {
  issue: Issue | null;
  /** Page to open on - an article link names one, otherwise the last page read. */
  startPage?: number | null;
  onClose: () => void;
}

/** Backend failures name torrents, archive paths and byte counts. The panel
 *  says what it means for the reader; the raw text goes to the console. */
function failureDetail(raw: string | null | undefined): string {
  const text = raw ?? "";
  if (/has not enabled/.test(text)) {
    return "This issue belongs to a collection this install does not have.";
  }
  if (/Not enough disk space/i.test(text)) {
    return "There is not enough free disk space for this issue.";
  }
  if (/timed out/i.test(text)) {
    return "No peers for this file yet. Try again in a moment.";
  }
  if (/is not in/.test(text)) {
    return "This issue is not in the collection's archive.";
  }
  return "Something went wrong while fetching this issue.";
}

/** The wait before the first page: a whole issue has to reach the disk over
 *  the torrent, so the panel says what is happening, how far it is, and
 *  offers the way out for each phase. */
function FetchPanel(p: {
  issue: Issue;
  status: ReadingStatus | undefined;
  onCancel: () => void;
  onRetry: () => void;
}) {
  let coverRef: HTMLDivElement | undefined;
  const cover = createKeyedCover(() => MEDIA_SOURCE, () => p.issue.cover_key, () => p.issue.key, () => coverRef);

  // An offline request is provisional and never kept in the store (§14), so
  // "no status" while offline is the offline answer, not "preparing".
  const phase = () => p.status?.phase ?? (isOffline() ? "none" : undefined);
  const inFlight = () => phase() === "fetching" || phase() === PHASE_QUEUED;
  const failed = () => phase() === "error" || phase() === "none";
  const total = () => p.status?.total_bytes || p.issue.size_bytes;
  const progress = () => p.status?.progress ?? 0;

  createEffect(on(() => p.status?.error, (raw) => {
    if (raw && p.status?.phase === "error") { console.error("[reading] fetch failed:", raw); }
  }));

  const headline = () => {
    switch (phase()) {
      case undefined: return "Preparing…";
      case PHASE_QUEUED: return "Waiting for a download slot";
      case "fetching": return "Downloading…";
      case "ready": return "Opening…";
      case "error": return "Couldn't fetch this issue";
      default: return isOffline() ? "Offline" : "Not available right now";
    }
  };

  const detail = () => {
    switch (phase()) {
      case PHASE_QUEUED:
        return "Starts as soon as another download finishes.";
      case "fetching":
        return progress() > 0
          ? `${formatBytes(progress() * total())} of ${formatBytes(total())}`
          : `Starting · ${formatBytes(total())}`;
      case "error":
        return failureDetail(p.status?.error);
      case "none":
        return isOffline()
          ? "Exodium is offline. Switch to online in Settings → Network to read this."
          : "This issue could not be fetched. Try again in a moment.";
      default:
        return "";
    }
  };

  return (
    <div class="document-viewer-status" data-testid="issue-fetch" data-phase={phase() ?? "preparing"}>
      <div class="issue-fetch">
        <div ref={coverRef} class="issue-fetch-cover">
          <Show when={cover.src()}>
            <img src={cover.src()!} alt="" onError={cover.onError} />
          </Show>
        </div>
        <div class="issue-fetch-body">
          <div class="issue-fetch-headline">{headline()}</div>
          <Show when={detail()}>
            <div class="issue-fetch-detail">{detail()}</div>
          </Show>
          <Show when={inFlight()}>
            <div class="issue-fetch-progress">
              <AutoProgress value={progress()} indeterminate={phase() === PHASE_QUEUED} />
              <span class="issue-fetch-pct">
                {phase() === "fetching" && progress() > 0 ? `${Math.round(progress() * 100)}%` : ""}
              </span>
            </div>
          </Show>
          <div class="issue-fetch-actions">
            <Show when={inFlight()}>
              <button class="btn-secondary" onClick={p.onCancel}>Cancel</button>
            </Show>
            <Show when={failed() && !isOffline()}>
              <button class="lib-empty-btn" onClick={p.onRetry}>Try again</button>
            </Show>
            <Show when={failed()}>
              <button class="btn-secondary" onClick={p.onCancel}>Close</button>
            </Show>
          </div>
        </div>
      </div>
    </div>
  );
}

export function IssueReader(props: IssueReaderProps) {
  const status = () => (props.issue ? issueStatus(props.issue.key) : undefined);
  const [confirmRemove, setConfirmRemove] = createSignal(false);

  // Only the issue may re-run this: `requestIssue` writes the very state the
  // panel reads, so a tracked call re-triggers itself without end.
  createEffect(on(() => props.issue, (issue) => {
    if (issue) { void requestIssue(issue.key); }
  }));

  /** Six PC World rows are a cover scan, not a document; pdf.js can only
   *  fail on those. The path is known once the fetch finished. A memo with
   *  its own equality, or every progress tick would hand the viewer a new
   *  object and remount the document. */
  const source = createMemo(
    () => {
      const path = status()?.path;
      if (!path) { return null; }
      return { kind: props.issue?.entry_kind === "image" ? "image" as const : "pdf" as const, path };
    },
    undefined,
    { equals: (a, b) => a?.kind === b?.kind && a?.path === b?.path },
  );

  const close = () => {
    const issue = props.issue;
    // A fetch still running - or still queued, or between the click and the
    // backend's first answer - is abandoned deliberately: it holds (or is
    // waiting for) one of three slots and nobody is reading.
    const phase = status()?.phase;
    const busy = phase === "fetching" || phase === PHASE_QUEUED;
    if (issue && (busy || isFetchInFlight(issue.key))) { void abortIssue(issue.key); }
    props.onClose();
  };

  const retry = () => {
    const issue = props.issue;
    if (issue) { void requestIssue(issue.key); }
  };

  /** Only for a document that is here - there is nothing to give back
   *  otherwise, and the reader is closed after it: its source is gone. */
  const onDisk = () => {
    const issue = props.issue;
    return issue != null && (isOnDisk(issue.key) || status()?.phase === "ready");
  };

  const remove = async () => {
    const issue = props.issue;
    if (!issue) { return; }
    await removeIssue(issue);
    props.onClose();
  };

  return (
    <Show when={props.issue}>
      {(issue) => (
        <>
        <DocumentViewer
          open={true}
          title={issue().title}
          subtitle={<>
            {issue().publication}
            <Show when={issue().year}>{(year) => <> · {year()}</>}</Show>
            {" · "}{kindLabel(issue())}{" · "}{formatBytes(issue().size_bytes)}
          </>}
          source={source()}
          placeholder={<FetchPanel issue={issue()} status={status()} onCancel={close} onRetry={retry} />}
          initialPage={props.startPage ?? issue().last_page ?? 1}
          onPage={(page) => rememberPage(issue().key, page)}
          actions={
            <Show when={onDisk()}>
              <button
                class="document-viewer-btn is-danger"
                data-testid="issue-reader-remove"
                onClick={() => setConfirmRemove(true)}
                title="Delete the downloaded file; the issue stays in the catalogue"
              >
                Remove from disk
              </button>
            </Show>
          }
          onClose={close}
          testId="issue-reader"
        />
        <ConfirmDialog
          open={confirmRemove()}
          title="Remove from disk"
          message={`Delete the downloaded copy of "${issue().title}"? You can download it again whenever you like.`}
          confirmLabel="Remove"
          danger
          onConfirm={() => void remove()}
          onClose={() => setConfirmRemove(false)}
        />
        </>
      )}
    </Show>
  );
}
