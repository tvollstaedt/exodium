import { describe, it, expect, afterEach } from "vitest";
import { trackWindowActivity } from "./windowActivity";

const root = document.documentElement;
const realHasFocus = document.hasFocus;
const setFocus = (focused: boolean) => { document.hasFocus = () => focused; };

describe("trackWindowActivity", () => {
  let stop: (() => void) | null = null;

  afterEach(() => {
    stop?.();
    stop = null;
    root.classList.remove("app-inactive");
    document.hasFocus = realHasFocus;
  });

  it("starts active and follows window focus", () => {
    stop = trackWindowActivity();
    expect(root.classList.contains("app-inactive")).toBe(false);

    setFocus(false);
    window.dispatchEvent(new Event("blur"));
    expect(root.classList.contains("app-inactive")).toBe(true);

    setFocus(true);
    window.dispatchEvent(new Event("focus"));
    expect(root.classList.contains("app-inactive")).toBe(false);
  });

  it("ignores a blur while the document, e.g. its iframe, still has focus", () => {
    stop = trackWindowActivity();
    setFocus(true);
    window.dispatchEvent(new Event("blur"));
    expect(root.classList.contains("app-inactive")).toBe(false);
  });

  it("stays inactive when the window comes back visible without focus", () => {
    stop = trackWindowActivity();
    setFocus(false);
    document.dispatchEvent(new Event("visibilitychange"));
    expect(root.classList.contains("app-inactive")).toBe(true);

    setFocus(true);
    document.dispatchEvent(new Event("visibilitychange"));
    expect(root.classList.contains("app-inactive")).toBe(false);
  });

  it("starts inactive when the document opens hidden", () => {
    const hidden = Object.getOwnPropertyDescriptor(Document.prototype, "hidden")!;
    Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
    stop = trackWindowActivity();
    expect(root.classList.contains("app-inactive")).toBe(true);
    delete (document as unknown as { hidden?: boolean }).hidden;
    Object.defineProperty(Document.prototype, "hidden", hidden);
  });

  it("stops listening once disposed", () => {
    trackWindowActivity()();
    setFocus(false);
    window.dispatchEvent(new Event("blur"));
    expect(root.classList.contains("app-inactive")).toBe(false);
  });
});
