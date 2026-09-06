import { createSignal, type Accessor } from "solid-js";
import { getConfig, setConfig } from "../api/tauri";

/** A reactive preference backed by the `config` table: unknown until
 *  loaded (no flash of a default), loaded once per session, written
 *  signal-first, failures silent. */
export interface ConfigSignal<T> {
  /** The current value. Equals `fallback` until `ensureLoaded` resolves. */
  value: Accessor<T>;
  /** False until the stored value has arrived. */
  loaded: Accessor<boolean>;
  /** Start the one-time read. Safe to call from every mount. */
  ensureLoaded: () => void;
  /** Update in memory, then persist. Never rejects. */
  set: (next: T) => Promise<void>;
}

export function createConfigSignal<T>(
  key: string,
  fallback: T,
  parse: (raw: string | null) => T,
  serialize: (value: T) => string,
): ConfigSignal<T> {
  const [value, setValue] = createSignal<T>(fallback);
  const [loaded, setLoaded] = createSignal(false);
  let loading: Promise<void> | null = null;

  const ensureLoaded = () => {
    if (loading) { return; }
    loading = getConfig(key)
      .then((raw) => { setValue(() => parse(raw)); })
      // An unreadable config must not resurrect a choice the user already made.
      .catch(() => { setValue(() => fallback); })
      .finally(() => { setLoaded(true); });
  };

  const set = async (next: T) => {
    setValue(() => next);
    try {
      await setConfig(key, serialize(next));
    } catch {
      // Holds for this session either way; nagging would be worse.
    }
  };

  return { value, loaded, ensureLoaded, set };
}

/** The two shapes actually stored today: a flag, and a set of keys. */
export const BOOL_CODEC = {
  parse: (raw: string | null) => raw === "1",
  serialize: (v: boolean) => (v ? "1" : "0"),
};

export const KEY_LIST_CODEC = {
  parse: (raw: string | null) => (raw ? raw.split(",").filter(Boolean) : []),
  serialize: (v: string[]) => v.join(","),
};
