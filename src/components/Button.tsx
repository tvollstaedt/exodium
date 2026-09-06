import { Show, splitProps, type JSX } from "solid-js";

/** Action-button variants, mapped to the existing stylesheet classes so the
 *  look is unchanged - this centralises behaviour, not design.
 *
 *  - `primary`   the one obvious action of a dialog or step
 *  - `secondary` its counterpart (Back, Cancel)
 *  - `small`     dense rows: content packs, settings
 *  - `danger`    destructive, always paired with a confirmation
 *  - `action`    the detail panel's action bar
 *  - `icon`      square icon-only button in the top bar
 */
export type ButtonVariant = "primary" | "secondary" | "small" | "danger" | "action" | "icon";

const VARIANT_CLASS: Record<ButtonVariant, string> = {
  primary: "btn-primary",
  secondary: "btn-secondary",
  small: "btn-small",
  danger: "btn-danger",
  action: "game-detail-btn",
  icon: "icon-btn",
};

interface ButtonProps extends JSX.ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  /** Spinner plus disabled: an in-flight action is never clickable twice. */
  loading?: boolean;
  /** Replacement label while loading; defaults to the normal children. */
  loadingLabel?: JSX.Element;
}

/** Every action button: one place for the disabled and loading states. */
export function Button(props: ButtonProps) {
  const [own, rest] = splitProps(props, ["variant", "loading", "loadingLabel", "class", "children", "disabled"]);
  const variantClass = () => VARIANT_CLASS[own.variant ?? "small"];

  return (
    <button
      {...rest}
      // `app-btn` carries the layout every variant needs (centred content, an
      // icon that sits on the text's middle rather than its baseline). The
      // variant classes carry only colour and size.
      class={`app-btn ${variantClass()}${own.class ? ` ${own.class}` : ""}`}
      disabled={own.disabled || own.loading}
      aria-busy={own.loading || undefined}
    >
      <Show when={own.loading} fallback={own.children}>
        <span class="btn-spinner" />
        {own.loadingLabel ?? own.children}
      </Show>
    </button>
  );
}
