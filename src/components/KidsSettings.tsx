import { createEffect, createSignal, For, on, onMount, Show } from "solid-js";
import { kidsChangePin, kidsDisable, kidsEnable, kidsUpdate } from "../api/tauri";
import { kidsEnabled, kidsStatus, loadKidsStatus, setKidsStatus } from "../stores/kids";
import { loadPlaylists, userPlaylists } from "../stores/playlists";
import { showToast } from "../stores/toasts";
import { languageName } from "../util";
import { Button } from "./Button";
import { SettingRow } from "./SettingRow";

const LANGUAGES = ["EN", "DE", "ES", "PL", "FR", "IT"];
const PIN_RULE = /^\d{4,8}$/;

function PinInput(props: { value: string; onInput: (v: string) => void; placeholder: string; testId: string }) {
  return (
    <input
      class="kids-pin-field"
      data-testid={props.testId}
      type="password"
      inputMode="numeric"
      autocomplete="off"
      maxLength={8}
      placeholder={props.placeholder}
      value={props.value}
      onInput={(e) => props.onInput(e.currentTarget.value.replace(/\D/g, ""))}
    />
  );
}

/** Settings → Kids mode (§26). Reachable only unlocked, so playlist and
 *  language change without the PIN; turning it off or a new PIN need it. */
export function KidsSettings() {
  const [playlist, setPlaylist] = createSignal("");
  const [language, setLanguage] = createSignal("EN");
  const [pin, setPin] = createSignal("");
  const [pin2, setPin2] = createSignal("");
  const [oldPin, setOldPin] = createSignal("");
  const [newPin, setNewPin] = createSignal("");
  const [offPin, setOffPin] = createSignal("");
  const [error, setError] = createSignal("");

  onMount(() => { void loadKidsStatus(); void loadPlaylists(); });
  createEffect(on(kidsStatus, (s) => {
    if (s?.enabled) {
      setPlaylist(s.playlist_id != null ? String(s.playlist_id) : "");
      setLanguage(s.language);
    }
  }));

  const fail = (e: unknown) => setError(e instanceof Error ? e.message : String(e));

  const enable = async () => {
    setError("");
    if (!PIN_RULE.test(pin())) { setError("The PIN must be 4 to 8 digits"); return; }
    if (pin() !== pin2()) { setError("The two PINs differ"); return; }
    try {
      setKidsStatus(await kidsEnable(pin(), playlist() ? Number(playlist()) : null, language()));
      void loadPlaylists();
      setPin("");
      setPin2("");
      showToast("Kids mode is on - it locks at every start, or now with the lock in the top bar", "success");
    } catch (e) { fail(e); }
  };

  const update = async (nextPlaylist: string, nextLanguage: string) => {
    setError("");
    try {
      setKidsStatus(await kidsUpdate(Number(nextPlaylist), nextLanguage));
    } catch (e) { fail(e); }
  };

  const changePin = async () => {
    setError("");
    if (!PIN_RULE.test(newPin())) { setError("The new PIN must be 4 to 8 digits"); return; }
    try {
      await kidsChangePin(oldPin(), newPin());
      setOldPin("");
      setNewPin("");
      showToast("PIN changed", "success");
    } catch (e) { fail(e); }
  };

  const disable = async () => {
    setError("");
    try {
      setKidsStatus(await kidsDisable(offPin()));
      setOffPin("");
      showToast("Kids mode is off", "success");
    } catch (e) { fail(e); }
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
        Every start locks; the PIN opens the rest. A child lock, not a safe: it keeps kids from wandering off, not
        someone determined.
      </p>

      <SettingRow label="Playlist" hint="Its installed games are what the kids screen shows.">
        <PlaylistSelect
          allowNew={!kidsEnabled()}
          onChange={(v) => { setPlaylist(v); if (kidsEnabled()) { void update(v, language()); } }}
        />
      </SettingRow>
      <SettingRow label="Language" hint="The version a click starts, where installed - otherwise English.">
        <LanguageSelect onChange={(v) => { setLanguage(v); if (kidsEnabled()) { void update(playlist(), v); } }} />
      </SettingRow>

      <Show
        when={kidsEnabled()}
        fallback={
          <SettingRow label="PIN" hint="4 to 8 digits, twice.">
            <div class="kids-pin-row">
              <PinInput value={pin()} onInput={setPin} placeholder="PIN" testId="kids-pin-new" />
              <PinInput value={pin2()} onInput={setPin2} placeholder="Repeat" testId="kids-pin-repeat" />
              <Button variant="small" data-testid="kids-enable" onClick={() => void enable()}>Turn on</Button>
            </div>
          </SettingRow>
        }
      >
        <SettingRow label="Change PIN">
          <div class="kids-pin-row">
            <PinInput value={oldPin()} onInput={setOldPin} placeholder="Current" testId="kids-pin-old" />
            <PinInput value={newPin()} onInput={setNewPin} placeholder="New" testId="kids-pin-changed" />
            <Button variant="small" onClick={() => void changePin()}>Change</Button>
          </div>
        </SettingRow>
        <SettingRow label="Turn off" hint="Forgot the PIN? Start Exodium once with --reset-kids-mode.">
          <div class="kids-pin-row">
            <PinInput value={offPin()} onInput={setOffPin} placeholder="PIN" testId="kids-pin-off" />
            <Button variant="small" data-testid="kids-disable" onClick={() => void disable()}>Turn off</Button>
          </div>
        </SettingRow>
      </Show>
      <Show when={error()}>
        <div class="playlist-name-error" data-testid="kids-settings-error">{error()}</div>
      </Show>
    </section>
  );
}
