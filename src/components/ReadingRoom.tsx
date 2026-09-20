import { createSignal, createMemo, createEffect, Index, Show, onMount } from "solid-js";
import { Portal } from "solid-js/web";
import { type Issue } from "../api/tauri";
import {
  publications,
  issues,
  loaded,
  fetches,
  isOnDisk,
  loadReadingCatalog,
  installIssue,
  launchIssue,
  abortIssue,
  toggleIssueFavorite,
  removeIssue,
  sortIssues,
  sectionOf,
  displayTitle,
  kindLabel,
  filterIssues,
  publicationGroups,
  KINDS,
  LANGUAGES,
  PHASE_QUEUED,
  type ReadingSort,
  type Kind,
  type ReadingLanguage,
} from "../stores/reading";
import { viewMode, applyViewMode, type ViewMode } from "../stores/view";
import { showToast } from "../stores/toasts";
import { isOffline } from "../stores/network";
import { MEDIA_SOURCE } from "../stores/thumbnails";
import { formatBytes, jumpBarDisplayLabel } from "../util";
import { createKeyedCover } from "./cover";
import { IssueReader } from "./IssueReader";
import { ConfirmDialog } from "./ConfirmDialog";
import { Select } from "./Select";
import { ViewToggle } from "./ViewToggle";
import { CircularProgress } from "./ProgressBar";
import { IconEmptyReading } from "./icons";
import { MediaNoticeDialog, loadMediaNotice, needsMediaNotice } from "./MediaNotice";

/** Browse the Media Pack: magazines, books and catalogs (§19). The catalogue
 *  is bundled, so this list is complete offline - only opening an issue needs
 *  the torrent. Same grid, list, toolbar and jump bar as Browse. */

const NOUNS: Record<Kind, [string, string]> = {
  all: ["document", "documents"],
  magazine: ["issue", "issues"],
  book: ["book", "books"],
  catalog: ["catalog", "catalogs"],
};

const SORT_OPTIONS: { value: ReadingSort; label: string }[] = [
  { value: "publication", label: "By publication" },
  { value: "title", label: "Title A–Z" },
  { value: "year_desc", label: "Newest first" },
  { value: "year_asc", label: "Oldest first" },
];

/** List columns; a column without `desc` sorts one way only. */
const LIST_COLUMNS: { label: string; cls: string; asc?: ReadingSort; desc?: ReadingSort }[] = [
  { label: "Title", cls: "row-title", asc: "title", desc: "title_desc" },
  { label: "Publication", cls: "row-pub", asc: "publication" },
  { label: "Year", cls: "row-year", asc: "year_asc", desc: "year_desc" },
  { label: "Kind", cls: "row-kind" },
  { label: "Size", cls: "row-size", asc: "size", desc: "size_desc" },
  { label: "Status", cls: "row-status" },
];

/** Sticky offset of the section separators: tab bar + toolbar, the same
 *  stack Browse uses. */
const SEPARATOR_TOP = 100;

interface Section {
  label: string;
  issues: Issue[];
}

interface ItemProps {
  issue: Issue;
  /** The publication is in view (section or filter), so titles drop it. */
  publicationInView: boolean;
  showKind: boolean;
  /** Both languages are in view, so a German issue says so. */
  showLanguage: boolean;
  onOpen: (issue: Issue) => void;
  onPlay: (issue: Issue) => void;
  onMenu: (issue: Issue, e: MouseEvent) => void;
}

const isBusy = (issue: Issue) => {
  const phase = fetches()[issue.key]?.phase;
  return phase === "fetching" || phase === PHASE_QUEUED;
};

/** The one-line state under a title, in the grid's action-label vocabulary. */
function actionOf(issue: Issue): { cls: string; text: string } {
  const state = fetches()[issue.key];
  if (state?.phase === PHASE_QUEUED) { return { cls: "action-downloading", text: "Queued" }; }
  if (state?.phase === "fetching") {
    return {
      cls: "action-downloading",
      text: state.progress > 0 ? `${Math.round(state.progress * 100)}%` : "Fetching…",
    };
  }
  if (issue.runnable) {
    if (issue.installed) { return { cls: "action-installed", text: "▶ Play" }; }
    return isOffline()
      ? { cls: "action-offline", text: "Not installed" }
      : { cls: "action-download", text: "↓ Download" };
  }
  // On disk reads as installed, the same word the grid uses for a game.
  const here = isOnDisk(issue.key);
  if (issue.last_page && issue.last_page > 1) {
    return { cls: here ? "action-installed" : "action-resume", text: `Continue · p. ${issue.last_page}` };
  }
  if (here) { return { cls: "action-installed", text: "Read" }; }
  if (isOffline()) { return { cls: "action-offline", text: "Offline" }; }
  return { cls: "action-download", text: "↓ Read" };
}

const favTitle = (issue: Issue) =>
  issue.favorited ? "Remove from favorites" : "Add to favorites";
const playTitle = (issue: Issue) =>
  issue.installed ? "Run this disk magazine in DOSBox" : "Download this disk magazine";

/** A 400-page colour scan really is 200 MB, so the number gets a sentence. */
const sizeTitle = (issue: Issue) =>
  `${formatBytes(issue.size_bytes)} — downloaded once, then kept until you remove it`;

/** Downloaded, in either shape: a cached document or an extracted disk
 *  magazine. What "Remove from disk" is offered for. */
const issueOnDisk = (issue: Issue) =>
  issue.runnable ? issue.installed : isOnDisk(issue.key);

function IssueCard(p: ItemProps) {
  let ref: HTMLDivElement | undefined;
  const cover = createKeyedCover(() => MEDIA_SOURCE, () => p.issue.cover_key, () => p.issue.key, () => ref);
  const state = () => fetches()[p.issue.key];
  const progress = () => state()?.progress ?? 0;
  const busy = () => isBusy(p.issue);
  const action = () => actionOf(p.issue);
  const activate = () => (p.issue.runnable ? p.onPlay(p.issue) : p.onOpen(p.issue));

  return (
    <div
      ref={ref}
      class="game-card issue-card"
      classList={{ installed: p.issue.installed, runnable: p.issue.runnable }}
      data-testid="issue-card"
      data-issue-key={p.issue.key}
      onClick={activate}
      onContextMenu={(e) => p.onMenu(p.issue, e)}
    >
      <div class="game-card-art">
        <Show when={cover.src()}>
          <img class="game-card-thumb" src={cover.src()!} alt="" onError={cover.onError} />
        </Show>
        <Show when={busy()}>
          <div class="game-card-download-overlay">
            <CircularProgress value={progress()} size={64} strokeWidth={5} indeterminate={state()?.phase === PHASE_QUEUED}>
              <Show when={progress() > 0} fallback={<span class="circular-progress-pct muted">…</span>}>
                <span class="circular-progress-pct">{Math.round(progress() * 100)}%</span>
              </Show>
            </CircularProgress>
            <button
              class="game-card-overlay-cancel"
              title="Cancel download"
              onClick={(e) => { e.stopPropagation(); void abortIssue(p.issue.key); }}
            >✕</button>
          </div>
        </Show>
        <div class="game-card-body">
          <div class="game-card-title">{displayTitle(p.issue, p.publicationInView)}</div>
          <div class="game-card-meta">
            <Show when={p.issue.year}>{(year) => <span>{year()}</span>}</Show>
            <Show when={!p.publicationInView}><span class="genre">{p.issue.publication}</span></Show>
          </div>
          <div class="game-card-footer">
            <Show when={p.issue.runnable}><span class="badge badge-disk">Disk mag</span></Show>
            <Show when={p.showLanguage && p.issue.language === "DE"}>
              <span class="badge badge-lang">DE</span>
            </Show>
            <Show when={p.showKind && p.issue.kind !== "magazine"}>
              <span class="badge badge-platform">{p.issue.kind}</span>
            </Show>
            <Show when={isOnDisk(p.issue.key) && !p.issue.runnable}>
              <span class="badge badge-ondisk" title="Downloaded - opens without waiting">On disk</span>
            </Show>
          </div>
          <div class="game-card-action-bar">
            <Show
              when={p.issue.runnable}
              fallback={<span class={`card-action-label ${action().cls}`}>{action().text}</span>}
            >
              <button
                class={`card-action-label issue-play ${action().cls}`}
                data-testid="issue-play"
                title={playTitle(p.issue)}
                disabled={busy()}
                onClick={(e) => { e.stopPropagation(); p.onPlay(p.issue); }}
              >
                {action().text}
              </button>
            </Show>
            <span class="card-action-size" title={sizeTitle(p.issue)}>{formatBytes(p.issue.size_bytes)}</span>
          </div>
        </div>
      </div>

      <button
        class={`favorite-btn${p.issue.favorited ? " is-favorited" : ""}`}
        title={favTitle(p.issue)}
        onClick={(e) => { e.stopPropagation(); void toggleIssueFavorite(p.issue); }}
      >
        <span class="fav-star">★</span>
      </button>
    </div>
  );
}

function IssueRow(p: ItemProps) {
  let ref: HTMLDivElement | undefined;
  const cover = createKeyedCover(() => MEDIA_SOURCE, () => p.issue.cover_key, () => p.issue.key, () => ref);
  const busy = () => isBusy(p.issue);
  const action = () => actionOf(p.issue);
  const activate = () => (p.issue.runnable ? p.onPlay(p.issue) : p.onOpen(p.issue));

  return (
    <div
      ref={ref}
      class="game-row issue-row"
      classList={{ installed: p.issue.installed }}
      data-testid="issue-row"
      data-issue-key={p.issue.key}
      onClick={activate}
      onContextMenu={(e) => p.onMenu(p.issue, e)}
    >
      <button
        class={`row-fav${p.issue.favorited ? " is-favorited" : ""}`}
        title={favTitle(p.issue)}
        onClick={(e) => { e.stopPropagation(); void toggleIssueFavorite(p.issue); }}
      >★</button>
      <span class="row-cover">
        <Show when={cover.src()}>
          <img src={cover.src()!} alt="" onError={cover.onError} />
        </Show>
      </span>
      <span class="row-title" title={p.issue.title}>
        <span class="row-title-text">{displayTitle(p.issue, p.publicationInView)}</span>
        <Show when={p.issue.runnable}><span class="badge badge-disk">Disk</span></Show>
        <Show when={p.showLanguage && p.issue.language === "DE"}>
          <span class="badge badge-lang">DE</span>
        </Show>
      </span>
      <span class="row-pub" title={p.issue.publication}>{p.issue.publication}</span>
      <span class="row-year">{p.issue.year ?? ""}</span>
      <span class="row-kind">{kindLabel(p.issue)}</span>
      <span class="row-size" title={sizeTitle(p.issue)}>{formatBytes(p.issue.size_bytes)}</span>
      <span class="row-status">
        <Show
          when={p.issue.runnable && !busy()}
          fallback={<span class={`card-action-label ${action().cls}`}>{action().text}</span>}
        >
          <button
            class={`card-action-label issue-play ${action().cls}`}
            data-testid="issue-play"
            title={playTitle(p.issue)}
            onClick={(e) => { e.stopPropagation(); p.onPlay(p.issue); }}
          >
            {action().text}
          </button>
        </Show>
        <Show when={busy()}>
          <button
            class="row-cancel"
            title="Cancel download"
            onClick={(e) => { e.stopPropagation(); void abortIssue(p.issue.key); }}
          >✕</button>
        </Show>
      </span>
    </div>
  );
}

interface ReadingRoomProps {
  /** The top bar's search box; filters titles and publication names. */
  query: string;
}

export function ReadingRoom(props: ReadingRoomProps) {
  let rootRef: HTMLDivElement | undefined;
  const [kind, setKind] = createSignal<Kind>("all");
  const [language, setLanguage] = createSignal<ReadingLanguage>("all");
  const [publicationId, setPublicationId] = createSignal<number | null>(null);
  const [sortBy, setSortBy] = createSignal<ReadingSort>("publication");
  const [reading, setReading] = createSignal<Issue | null>(null);
  /** What the shared media notice (§19) was raised for, if anything. */
  const [noticeFor, setNoticeFor] = createSignal<Issue | null>(null);
  // Same shape as the grid's right-click menu on a game: the destructive
  // action lives there, not on the card.
  const [menu, setMenu] = createSignal<{ x: number; y: number; issue: Issue } | null>(null);
  const [confirmRemove, setConfirmRemove] = createSignal<Issue | null>(null);

  onMount(() => {
    void loadReadingCatalog().catch((e) => console.error("[reading] catalogue failed to load:", e));
    void loadMediaNotice();
  });

  // A kind or language switch invalidates a publication picked under the old one.
  createEffect(() => {
    const activeKind = kind();
    const activeLanguage = language();
    const current = publicationId();
    if (current == null) { return; }
    const publication = publications().find((p) => p.id === current);
    if (!publication) { return; }
    if ((activeKind !== "all" && publication.kind !== activeKind) ||
        (activeLanguage !== "all" && publication.language !== activeLanguage)) {
      setPublicationId(null);
    }
  });

  const publicationOptions = createMemo(() => {
    const options: { value: string; label: string; triggerLabel?: string; header?: boolean }[] = [
      { value: "", label: "All publications" },
    ];
    for (const group of publicationGroups(publications(), kind(), language())) {
      if (group.label) {
        options.push({ value: `group:${group.label}`, label: group.label, header: true });
      }
      for (const p of group.rows) {
        options.push({ value: String(p.id), label: `${p.name} · ${p.issue_count}`, triggerLabel: p.name });
      }
    }
    return options;
  });

  const shownIssues = createMemo(() => {
    const rows = filterIssues(issues(), {
      kind: kind(),
      language: language(),
      publicationId: publicationId(),
      query: props.query,
    });
    return sortIssues(rows, sortBy());
  });

  const sections = createMemo<Section[]>(() => {
    const result: Section[] = [];
    let current: Section | null = null;
    for (const issue of shownIssues()) {
      const label = sectionOf(issue, sortBy());
      if (current === null || label !== current.label) {
        current = { label, issues: [] };
        result.push(current);
      }
      current.issues.push(issue);
    }
    return result;
  });

  const publicationInView = () => sortBy() === "publication" || publicationId() != null;
  const hasFilters = () => kind() !== "all" || language() !== "all" || publicationId() != null;
  const resultsLabel = () => {
    const n = shownIssues().length;
    return `${n.toLocaleString()} ${NOUNS[kind()][n === 1 ? 0 : 1]}`;
  };

  // Column sorts the grid's select cannot show fall back to its default.
  const switchView = (mode: ViewMode) => {
    applyViewMode(mode);
    if (mode === "grid" && !SORT_OPTIONS.some((o) => o.value === sortBy())) {
      setSortBy("publication");
    }
  };

  const sortByColumn = (col: typeof LIST_COLUMNS[number]) => {
    if (!col.asc) { return; }
    setSortBy(sortBy() === col.asc && col.desc ? col.desc : col.asc);
  };

  const columnIndicator = (col: typeof LIST_COLUMNS[number]) => {
    if (sortBy() === col.asc) { return " ▲"; }
    if (col.desc && sortBy() === col.desc) { return " ▼"; }
    return "";
  };

  /** Same measurement as Browse: the separator is sticky and reports its
   *  stuck position, so the grid sibling is what gets scrolled to. */
  const jumpToSection = (label: string) => {
    const el = rootRef?.querySelector<HTMLElement>(`[data-section-label="${CSS.escape(label)}"]`);
    const scroller = rootRef?.closest<HTMLElement>(".library");
    if (!el || !scroller) { return; }
    const grid = el.nextElementSibling as HTMLElement | null;
    const rect = (grid ?? el).getBoundingClientRect();
    const containerRect = scroller.getBoundingClientRect();
    scroller.scrollBy({
      top: rect.top - containerRect.top - SEPARATOR_TOP - (grid ? el.offsetHeight : 0),
      behavior: "smooth",
    });
  };

  const openIssue = (issue: Issue) => {
    if (issue.runnable) { return; }
    if (needsMediaNotice()) {
      setNoticeFor(issue);
      return;
    }
    setReading(issue);
  };

  /** A disk magazine's one action: fetch it, then run it. */
  const playIssue = async (issue: Issue) => {
    if (isBusy(issue)) { return; }
    if (!issue.installed) {
      if (isOffline()) { return; }
      if (needsMediaNotice()) {
        setNoticeFor(issue);
        return;
      }
      void installIssue(issue.key);
      return;
    }
    try {
      await launchIssue(issue.key);
    } catch (e) {
      showToast(`Couldn't launch ${issue.title}`, "error", {
        detail: String(e).replace(/^Error:\s*/, ""),
      });
    }
  };

  /** What the media notice was raised for: reading it, or downloading it. */
  const continueWithNotice = (issue: Issue) => {
    if (issue.runnable) {
      void installIssue(issue.key);
    } else {
      setReading(issue);
    }
  };

  const onPlay = (issue: Issue) => { void playIssue(issue); };

  const onMenu = (issue: Issue, e: MouseEvent) => {
    // Before the on-disk test: the webview's own menu is never wanted here,
    // and a card with nothing to remove would otherwise get it.
    e.preventDefault();
    if (!issueOnDisk(issue)) { return; }
    setMenu({ x: e.clientX, y: e.clientY, issue });
  };

  const performRemove = async (issue: Issue) => {
    try {
      await removeIssue(issue);
    } catch (e) {
      showToast(`Couldn't remove ${issue.title}`, "error", {
        detail: String(e).replace(/^Error:\s*/, ""),
      });
    }
  };

  return (
    <div class="reading-room" ref={rootRef}>
      <div class="library-toolbar">
        <div class="reading-kinds" role="group" aria-label="Kind">
          <Index each={KINDS}>
            {(entry) => (
              <button
                class="filter-chip"
                classList={{ active: kind() === entry().id }}
                data-testid={`reading-kind-${entry().id}`}
                onClick={() => setKind(entry().id)}
              >
                {entry().label}
              </button>
            )}
          </Index>
        </div>
        <div class="reading-kinds" role="group" aria-label="Language">
          <Index each={LANGUAGES}>
            {(entry) => (
              <button
                class="filter-chip"
                classList={{ active: language() === entry().id }}
                data-testid={`reading-lang-${entry().id.toLowerCase()}`}
                onClick={() => setLanguage(entry().id)}
              >
                {entry().label}
              </button>
            )}
          </Index>
        </div>
        <Select
          class="select-wide"
          options={publicationOptions()}
          value={publicationId() != null ? String(publicationId()) : ""}
          onChange={(value) => setPublicationId(value ? Number(value) : null)}
          placeholder="All publications"
        />
        <Show when={viewMode() === "grid"}>
          <Select
            options={SORT_OPTIONS}
            value={sortBy()}
            onChange={(value) => setSortBy(value as ReadingSort)}
            placeholder="Sort by"
          />
        </Show>
        <Show when={loaded()}>
          <span class="results-count" data-testid="reading-count">{resultsLabel()}</span>
        </Show>
        <ViewToggle mode={viewMode()} onChange={switchView} />
      </div>

      <Show when={isOffline()}>
        <div class="reading-note">
          Offline - the catalogue is here to browse; opening an issue needs a connection.
        </div>
      </Show>

      <Show when={loaded()} fallback={<div class="loading">Loading the reading room…</div>}>
        <Show
          when={shownIssues().length > 0}
          fallback={
            <div class="lib-empty">
              <div class="lib-empty-icon"><IconEmptyReading /></div>
              <div class="lib-empty-text">
                {props.query.trim()
                  ? `No documents match "${props.query.trim()}"`
                  : "No documents match these filters"}
              </div>
              <div class="lib-empty-sub">Try a different search or another publication</div>
              <Show when={hasFilters()}>
                <button class="lib-empty-btn" onClick={() => { setKind("all"); setLanguage("all"); setPublicationId(null); }}>
                  Show everything
                </button>
              </Show>
            </div>
          }
        >
          <Show when={viewMode() === "grid"}>
            <div class="sections-list" data-testid="reading-grid">
              <Index each={sections()}>
                {(section) => (
                  <>
                    <Show when={section().label}>
                      <div
                        data-section-label={section().label}
                        class="grid-separator"
                        style={{ top: `${SEPARATOR_TOP}px` }}
                      >
                        {section().label}
                        <span class="section-count">{section().issues.length}</span>
                      </div>
                    </Show>
                    <div class="game-grid game-section">
                      <Index each={section().issues}>
                        {(issue) => (
                          <IssueCard
                            issue={issue()}
                            publicationInView={publicationInView()}
                            showKind={kind() === "all"}
                            showLanguage={language() === "all"}
                            onOpen={openIssue}
                            onPlay={onPlay}
                            onMenu={onMenu}
                          />
                        )}
                      </Index>
                    </div>
                  </>
                )}
              </Index>
            </div>
          </Show>
          <Show when={viewMode() === "list"}>
            <div class="game-list" data-testid="reading-list">
              <div class="game-list-header issue-list-header" style={{ top: `${SEPARATOR_TOP}px` }}>
                <span class="row-fav" />
                <span class="row-cover" />
                <Index each={LIST_COLUMNS}>
                  {(col) => (
                    <button
                      class={`list-col ${col().cls}${col().asc ? " sortable" : ""}`}
                      disabled={!col().asc}
                      onClick={() => sortByColumn(col())}
                    >
                      {col().label}{columnIndicator(col())}
                    </button>
                  )}
                </Index>
              </div>
              <Index each={shownIssues()}>
                {(issue) => (
                  <IssueRow
                    issue={issue()}
                    publicationInView={publicationInView()}
                    showKind={kind() === "all"}
                    showLanguage={language() === "all"}
                    onOpen={openIssue}
                    onPlay={onPlay}
                    onMenu={onMenu}
                  />
                )}
              </Index>
            </div>
          </Show>
        </Show>
      </Show>

      <Show when={viewMode() === "grid" && sections().length > 1 && sections()[0].label}>
        <Portal>
          <div class="jump-bar">
            <Index each={sections()}>
              {(section) => (
                <button class="jump-bar-item" title={section().label} onClick={() => jumpToSection(section().label)}>
                  {jumpBarDisplayLabel(section().label)}
                </button>
              )}
            </Index>
          </div>
        </Portal>
      </Show>

      <Show when={menu()}>
        <Portal>
          <div
            class="context-backdrop"
            onMouseDown={() => setMenu(null)}
            onContextMenu={(e) => { e.preventDefault(); setMenu(null); }}
          />
          <div class="context-menu" style={{ left: `${menu()!.x}px`, top: `${menu()!.y}px` }}>
            <button
              class="context-menu-item danger"
              data-testid="issue-remove"
              title="Delete the downloaded files; the issue stays in the catalogue"
              onMouseDown={(e) => e.stopPropagation()}
              onClick={() => {
                const issue = menu()!.issue;
                setMenu(null);
                setConfirmRemove(issue);
              }}
            >
              Remove from disk
            </button>
          </div>
        </Portal>
      </Show>

      <ConfirmDialog
        open={confirmRemove() != null}
        title="Remove from disk"
        message={`Delete the downloaded copy of "${confirmRemove()?.title ?? ""}"? You can download it again whenever you like.`}
        confirmLabel="Remove"
        danger
        onConfirm={() => {
          const issue = confirmRemove();
          if (issue) { void performRemove(issue); }
        }}
        onClose={() => setConfirmRemove(null)}
      />

      <IssueReader issue={reading()} onClose={() => setReading(null)} />

      <MediaNoticeDialog
        open={noticeFor() != null}
        confirmLabel={noticeFor()?.runnable ? "Download the issue" : "Open the issue"}
        onConfirm={() => {
          const issue = noticeFor();
          setNoticeFor(null);
          if (issue) { continueWithNotice(issue); }
        }}
        onClose={() => setNoticeFor(null)}
      />
    </div>
  );
}
