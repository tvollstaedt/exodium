import { For } from "solid-js";
import { Portal } from "solid-js/web";
import { toasts, dismissToast, type Toast, type ToastType } from "../stores/toasts";

const ICONS: Record<ToastType, string> = {
  info: "i",
  success: "✓",
  warning: "!",
  error: "✕",
};

function ToastRow(props: { t: Toast }) {
  return (
    <div class={`toast toast-${props.t.type}`} data-testid="toast">
      <span class={`toast-icon toast-icon-${props.t.type}`}>{ICONS[props.t.type]}</span>
      <div class="toast-body">
        <div class="toast-message">{props.t.message}</div>
        {props.t.detail ? <div class="toast-detail">{props.t.detail}</div> : null}
        {props.t.action ? (
          <button class="toast-action" onClick={props.t.action.onClick}>
            {props.t.action.label}
          </button>
        ) : null}
      </div>
      <button
        class="toast-dismiss"
        onClick={() => dismissToast(props.t.id)}
        aria-label="Dismiss"
      >×</button>
    </div>
  );
}

// Two stacked regions: errors get aria-live="assertive" so screen readers
// preempt other speech, everything else is "polite". Using a single region
// with a dynamic aria-live is unreliable - some ATs latch the initial value.
export function ToastContainer() {
  const errors = () => toasts().filter((t) => t.type === "error");
  const others = () => toasts().filter((t) => t.type !== "error");

  return (
    <Portal>
      <div class="toast-stack">
        <div role="alert" aria-live="assertive">
          <For each={errors()}>{(t) => <ToastRow t={t} />}</For>
        </div>
        <div role="status" aria-live="polite">
          <For each={others()}>{(t) => <ToastRow t={t} />}</For>
        </div>
      </div>
    </Portal>
  );
}
