import { Show, type JSX } from "solid-js";
import { Switch } from "@ark-ui/solid/switch";

interface ToggleProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: JSX.Element;
  /** One line explaining the consequence, not restating the label. */
  hint?: JSX.Element;
  disabled?: boolean;
}

/** An on/off setting that applies immediately: Ark's Switch with the
 *  label/hint arrangement fixed in one place. */
export function Toggle(props: ToggleProps) {
  return (
    <Switch.Root
      class="setting-switch"
      checked={props.checked}
      disabled={props.disabled}
      onCheckedChange={(e) => props.onChange(e.checked)}
    >
      <Switch.Control class="setting-switch-control">
        <Switch.Thumb class="setting-switch-thumb" />
      </Switch.Control>
      <Switch.Label class="setting-toggle-info">
        <span class="setting-toggle-label">{props.label}</span>
        <Show when={props.hint}>
          <span class="setting-toggle-hint">{props.hint}</span>
        </Show>
      </Switch.Label>
      <Switch.HiddenInput />
    </Switch.Root>
  );
}
