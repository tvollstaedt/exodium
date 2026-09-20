import { createSignal } from "solid-js";
import { getConfig, setConfig } from "../api/tauri";
import { isOffline } from "../stores/network";
import { ConfirmDialog } from "./ConfirmDialog";

/** Opening an issue joins a second torrent, which - with seeding on - also
 *  distributes scanned magazines. Said once before the first fetch, from
 *  wherever it is started (§19). Information, not consent: the seeding
 *  decision itself lives in Settings → Network (§11). */

const [seen, setSeen] = createSignal(true);

let pending: Promise<void> | null = null;

/** Idempotent - every entry point asks, and the game panel mounts per game. */
export function loadMediaNotice(): Promise<void> {
  pending ??= getConfig("media_notice_seen")
    .then((value) => { setSeen(value === "1"); })
    .catch((e) => { console.warn("[reading] could not read media_notice_seen:", e); });
  return pending;
}

/** Offline nothing is transferred and no torrent is joined, so there is
 *  nothing to announce. */
export const needsMediaNotice = () => !seen() && !isOffline();

function accept() {
  setSeen(true);
  void setConfig("media_notice_seen", "1")
    .catch((e) => console.error("[reading] could not store media_notice_seen:", e));
}

export function MediaNoticeDialog(props: {
  open: boolean;
  confirmLabel: string;
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <ConfirmDialog
      open={props.open}
      title="Reading uses your connection"
      message="Magazines, books and catalogs are downloaded one at a time - only the issue you open is transferred. While seeding is enabled, Exodium also shares what it has downloaded. You can turn seeding off in Settings → Network."
      confirmLabel={props.confirmLabel}
      cancelLabel="Not now"
      onConfirm={() => { accept(); props.onConfirm(); }}
      onClose={props.onClose}
    />
  );
}
