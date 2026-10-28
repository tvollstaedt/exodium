import { createEffect, createSignal, For, on, onMount, Show } from "solid-js";
import { kidsEnable, kidsUpdate } from "../api/tauri";
import {
  changeKidsPin, disableKids, kidsEnabled, kidsStatus, loadKidsStatus, setKidsStatus, takeRequestedKidsPlaylist,
} from "../stores/kids";
import { loadPlaylists, userPlaylists } from "../stores/playlists";
import { showToast } from "../stores/toasts";
import { languageName } from "../util";
import { Button } from "./Button";
import { PinDialog, pinAnswer, type PinAnswer } from "./PinDialog";
import { SettingRow } from "./SettingRow";

const LANGUAGES = ["EN", "DE", "ES", "PL", "FR", "IT"];
const RESET_HINT = "Forgot it? Start Exodium once with --reset-kids-mode.";

const ON_FIELDS = [
  { testId: "kids-pin-new", placeholder: "PIN" },
  { testId: "kids-pin-repeat", placeholder: "Repeat PIN" },
];
const CHANGE_FIELDS = [
  { testId: "kids-pin-current", placeholder: "Current PIN" },
  { testId: "kids-pin-new", placeholder: "New PIN" },
  { testId: "kids-pin-repeat", placeholder: "Repeat new PIN" },
];
const OFF_FIELDS = [{ testId: "kids-pin-current", placeholder: "PIN" }];

/** Settings → Kids mode (§26). Reachable only unlocked, so playlist and
 *  language change without the PIN; turning it off or a new PIN need it.
 *  `onLock` is the lock button's path: Settings closes and the app locks. */
export function KidsSettings(props: { onLock: () => void }) {
  const [playlist, setPlaylist] = createSignal("");
  const [language, setLanguage] = createSignal("EN");
  const [dialog, setDialog] = createSignal<"on" | "change" | "off" | null>(null);
  const [error, setError] = createSignal("");

  createEffect(on(kidsStatus, (s) => {
    if (s?.enabled) {
      setPlaylist(s.playlist_id != null ? String(s.playlist_id) : "");
      setLanguage(s.language);
    }
  }));

  const update = async (nextPlaylist: string, nextLanguage: string) => {
    setError("");
    try {
      setKidsStatus(await kidsUpdate(Number(nextPlaylist), nextLanguage));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const choosePlaylist = (v: string) => {
    setPlaylist(v);
    if (kidsEnabled()) { void update(v, language()); }
  };

  const chooseLanguage = (v: string) => {
    setLanguage(v);
    if (kidsEnabled()) { void update(playlist(), v); }
  };

  // After the status: its effect would otherwise overwrite the requested pick.
  onMount(async () => {
    void loadPlaylists();
    await loadKidsStatus();
    const requested = takeRequestedKidsPlaylist();
    if (requested != null) { choosePlaylist(String(requested)); }
  });

  const turnOn = async ([pin]: string[]): Promise<PinAnswer> => {
    setKidsStatus(await kidsEnable(pin, playlist() ? Number(playlist()) : null, language()));
    void loadPlaylists();
    setDialog(null);
    props.onLock();
    return { ok: true };
  };

  const changePin = async ([current, next]: string[]): Promise<PinAnswer> => {
    const check = await changeKidsPin(current, next);
    if (check.ok) {
      setDialog(null);
      showToast("PIN changed", "success");
    }
    return pinAnswer(check);
  };

  const turnOff = async ([pin]: string[]): Promise<PinAnswer> => {
    const check = await disableKids(pin);
    if (check.ok) {
      setDialog(null);
      showToast("Kids mode is off", "success");
    }
    return pinAnswer(check);
  };

  const PlaylistSelect = (p: { allowNew: boolean; onChange: (v: string) => void }) => (
    <select class="game-settings-select" data-testid="kids-playlist" value={playlist()} onChange={(e) => p.onChange(e.currentTarget.value)}>
      <Show when={p.allowNew}><option value="" selected={playlist() === ""}>New playlist "Kids"</option></Show>
      <For each={userPlaylists()}>{(pl) => <option value={String(pl.id)} selected={String(pl.id) === playlist()}>{pl.name}</option>}</For>
    </select>
  );
  const LanguageSelect = (p: { onChange: (v: string) => void }) => (
    <select class="game-settings-select" data-testid="kids-language" value={language()} onChange={(e) => p.onChange(e.currentTarget.value)}>
      <For each={LANGUAGES}>{(l) => <option value={l} selected={l === language()}>{languageName(l)}</option>}</For>
    </select>
  );

  return (
    <section class="settings-section" data-testid="kids-settings">
      <h3 class="settings-section-title">Kids mode</h3>
      <p class="settings-section-hint">
        A locked screen with only the installed games of one playlist - no downloads, no settings, nothing else.
        It locks when turned on, at every start and with Lock now; the PIN opens the rest. A child lock, not a
        safe: it keeps kids from wandering off, not someone determined.
      </p>

      <SettingRow
        label="Kids mode"
        value={
          <span class={`kids-status-badge${kidsEnabled() ? " is-on" : ""}`} data-testid="kids-status">
            {kidsEnabled() ? "On" : "Off"}
          </span>
        }
      >
        <Show
          when={kidsEnabled()}
          fallback={<Button variant="small" data-testid="kids-turn-on" onClick={() => setDialog("on")}>Turn on…</Button>}
        >
          <Button variant="small" data-testid="kids-lock-now" onClick={props.onLock}>Lock now</Button>
          <Button variant="small" data-testid="kids-turn-off" onClick={() => setDialog("off")}>Turn off…</Button>
        </Show>
      </SettingRow>
      <SettingRow label="Playlist" hint="Its installed games are what the kids screen shows.">
        <PlaylistSelect allowNew={!kidsEnabled()} onChange={choosePlaylist} />
      </SettingRow>
      <SettingRow label="Language" hint="The version a click starts, where installed - otherwise English.">
        <LanguageSelect onChange={chooseLanguage} />
      </SettingRow>
      <Show when={kidsEnabled()}>
        <SettingRow label="PIN" hint={RESET_HINT}>
          <Button variant="small" data-testid="kids-change-pin" onClick={() => setDialog("change")}>Change PIN…</Button>
        </SettingRow>
      </Show>
      <Show when={error()}>
        <div class="playlist-name-error" data-testid="kids-settings-error">{error()}</div>
      </Show>

      <PinDialog
        open={dialog() === "on"}
        title="Turn on kids mode"
        description="Choose a PIN of 4 to 8 digits. Exodium locks right away and at every start; the PIN unlocks it."
        fields={ON_FIELDS}
        mismatch="The two PINs differ"
        submitLabel="Turn on and lock"
        testId="kids-turn-on-dialog"
        submitTestId="kids-enable"
        onSubmit={turnOn}
        onClose={() => setDialog(null)}
      />
      <PinDialog
        open={dialog() === "change"}
        title="Change PIN"
        fields={CHANGE_FIELDS}
        mismatch="The two new PINs differ"
        submitLabel="Change PIN"
        testId="kids-change-pin-dialog"
        submitTestId="kids-change-pin-submit"
        onSubmit={changePin}
        onClose={() => setDialog(null)}
      />
      <PinDialog
        open={dialog() === "off"}
        title="Turn off kids mode"
        description={`Exodium stops locking; the playlist stays. ${RESET_HINT}`}
        fields={OFF_FIELDS}
        submitLabel="Turn off"
        autoSubmit
        testId="kids-turn-off-dialog"
        submitTestId="kids-disable"
        onSubmit={turnOff}
        onClose={() => setDialog(null)}
      />
    </section>
  );
}
