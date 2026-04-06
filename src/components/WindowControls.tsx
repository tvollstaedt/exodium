import { getCurrentWindow } from "@tauri-apps/api/window";

export function WindowControls() {
  const win = getCurrentWindow();

  return (
    <div class="window-controls">
      <button class="win-btn win-minimize" onClick={() => win.minimize()}>
        <svg width="10" height="1" viewBox="0 0 10 1"><rect width="10" height="1" fill="currentColor"/></svg>
      </button>
      <button class="win-btn win-maximize" onClick={() => win.toggleMaximize()}>
        <svg width="10" height="10" viewBox="0 0 10 10"><rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" stroke-width="1"/></svg>
      </button>
      <button class="win-btn win-close" onClick={() => win.close()}>
        <svg width="10" height="10" viewBox="0 0 10 10"><line x1="0" y1="0" x2="10" y2="10" stroke="currentColor" stroke-width="1.2"/><line x1="10" y1="0" x2="0" y2="10" stroke="currentColor" stroke-width="1.2"/></svg>
      </button>
    </div>
  );
}
