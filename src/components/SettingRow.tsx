import { Show, children, type JSX, type Component } from "solid-js";
import type { LucideProps } from "lucide-solid";
import { Switch } from "@ark-ui/solid/switch";

/** One settings line: label column, body (value and one-line hint), action
 *  column. Every row in the dialog is this shape, so the columns line up.
 *  `htmlFor` binds the label to a control's hidden input, so clicking the
 *  words toggles it. */
export function SettingRow(props: {
  label: JSX.Element;
  value?: JSX.Element;
  hint?: JSX.Element;
  /** Body below the label instead of beside it - for wide controls. */
  stacked?: boolean;
  htmlFor?: string;
  children?: JSX.Element;
}) {
  // Resolved once: reading a JSX prop inside `Show`'s `when` would build it
  // a second time and leave the orphan's effects live.
  const value = children(() => props.value);
  const hint = children(() => props.hint);
  const action = children(() => props.children);
  return (
    <div class={`settings-row${props.stacked ? " is-stacked" : ""}`}>
      <Show when={props.htmlFor} fallback={<span class="settings-row-label">{props.label}</span>}>
        <label class="settings-row-label" for={props.htmlFor}>{props.label}</label>
      </Show>
      <div class="settings-row-body">
        <Show when={value()}><span class="settings-row-value">{value()}</span></Show>
        <Show when={hint()}><span class="settings-row-hint">{hint()}</span></Show>
      </div>
      <Show when={action()}><div class="settings-row-action">{action()}</div></Show>
    </div>
  );
}

/** The switch alone, for a row's action column; the row carries the words. */
export function SettingSwitch(props: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  /** Id of the hidden input, for the row label's `for`. */
  id?: string;
  disabled?: boolean;
}) {
  return (
    <Switch.Root
      class="settings-switch"
      checked={props.checked}
      disabled={props.disabled}
      ids={props.id ? { hiddenInput: props.id } : undefined}
      onCheckedChange={(e) => props.onChange(e.checked)}
    >
      <Switch.Control class="setting-switch-control">
        <Switch.Thumb class="setting-switch-thumb" />
      </Switch.Control>
      <Switch.Label class="visually-hidden">{props.label}</Switch.Label>
      <Switch.HiddenInput />
    </Switch.Root>
  );
}

/** Section heading with its Lucide glyph; the icon names the group, the
 *  uppercase text stays for scanning. */
export function SectionTitle(props: { icon: Component<LucideProps>; children: JSX.Element }) {
  return (
    <h3 class="settings-section-title">
      <props.icon size={14} strokeWidth={1.8} class="settings-title-icon" aria-hidden="true" />
      <span>{props.children}</span>
    </h3>
  );
}
