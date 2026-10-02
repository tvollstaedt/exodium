/** Marks the root `app-inactive` while the window has no focus, so main.css
 *  can stop endless animations nobody is looking at. Returns the disposer. */
export function trackWindowActivity(root: HTMLElement = document.documentElement): () => void {
  const set = (inactive: boolean) => { root.classList.toggle("app-inactive", inactive); };
  // Focus moving into the document viewer's iframe blurs the top window while
  // the document still has focus; only a document that lost it counts.
  const onBlur = () => { set(!document.hasFocus()); };
  const onFocus = () => { set(false); };
  const onVisibility = () => { set(document.hidden || !document.hasFocus()); };

  // A window that opens hidden has no focus event coming; one that merely lacks
  // focus at startup is left running, since a missed focus event would freeze it.
  set(document.hidden);
  window.addEventListener("blur", onBlur);
  window.addEventListener("focus", onFocus);
  document.addEventListener("visibilitychange", onVisibility);
  return () => {
    window.removeEventListener("blur", onBlur);
    window.removeEventListener("focus", onFocus);
    document.removeEventListener("visibilitychange", onVisibility);
  };
}
