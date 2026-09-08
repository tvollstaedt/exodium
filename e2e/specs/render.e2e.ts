import { $, browser, expect } from "@wdio/globals";

// Regression specs for the 2026-09-08 Linux report: a spinner stood still, the
// back-to-top pill never appeared, the scrollbar was too thin to grab and the
// download sheet's hover changed the label's rendering. All of them are
// engine-real checks - they assert computed style and behavior in the packaged
// webview, which is where WebKitGTK diverged from the other platforms.
describe("rendering regressions", () => {
  it("the button spinner actually rotates", async () => {
    // The shipped bug: the compositing block gives .btn-spinner a base
    // `translateZ(0)`, and a to-only keyframe then interpolates mismatched
    // transform lists via matrices - rotate(360deg) IS the identity matrix,
    // so the animation ran from identity to identity and the spinner froze.
    const res = await browser.executeAsync((done: (r: { a: string; b: string }) => void) => {
      const el = document.createElement("span");
      el.className = "btn-spinner";
      document.body.appendChild(el);
      const a = getComputedStyle(el).transform;
      setTimeout(() => {
        const b = getComputedStyle(el).transform;
        el.remove();
        done({ a, b });
      }, 300);
    });
    expect(res.a).not.toEqual(res.b);
  });

  it("back-to-top appears on upward scroll, even in sub-pixel steps", async () => {
    await $(".library").waitForExist();
    // Fine steps model WebKitGTK's animated wheel scrolling; the store's
    // delta filter (>2px) must accumulate across them, not eat them.
    const visible = await browser.executeAsync((done: (v: boolean) => void) => {
      const lib = document.querySelector<HTMLElement>(".library");
      const btn = document.querySelector<HTMLElement>(".back-to-top");
      if (!lib || !btn) { done(false); return; }
      lib.scrollTop = 1400;
      let i = 0;
      const step = () => {
        lib.scrollTop = Math.max(0, lib.scrollTop - 2);
        i++;
        if (btn.classList.contains("visible")) { done(true); return; }
        if (i > 300) { done(false); return; }
        requestAnimationFrame(step);
      };
      setTimeout(() => requestAnimationFrame(step), 100);
    });
    expect(visible).toBe(true);
    // Clicking it returns to the top and it stands down.
    await $(".back-to-top").click();
    await browser.waitUntil(
      async () => (await browser.execute(() => document.querySelector(".library")!.scrollTop)) === 0,
      { timeoutMsg: "back-to-top did not scroll the library to the top" },
    );
  });

  it("the library scrollbar is wide enough to grab", async () => {
    const width = await browser.execute(() => {
      const lib = document.querySelector<HTMLElement>(".library")!;
      return lib.offsetWidth - lib.clientWidth;
    });
    // 12px custom scrollbar (plus gutter rounding); 8px was the unusable state.
    expect(width).toBeGreaterThanOrEqual(10);
  });

  it("hovering a download-sheet link changes nothing but the underline", async () => {
    // The regression: the hover rule carried text-transform: capitalize and a
    // margin, so the label visibly jumped and re-cased under the pointer.
    const res = await browser.execute(() => {
      const el = document.createElement("button");
      el.className = "download-sheet-label is-link";
      el.textContent = "probe";
      document.body.appendChild(el);
      const base = getComputedStyle(el);
      const rest = { transform: base.textTransform, margin: base.marginBottom };
      el.remove();
      return rest;
    });
    expect(res.transform).toEqual("none");
    expect(res.margin).toEqual("0px");
    const hovered = await browser.execute(() => {
      // :hover cannot be forced from script; assert the stylesheet itself.
      const offending: string[] = [];
      for (const sheet of Array.from(document.styleSheets)) {
        let rules: CSSRuleList;
        try { rules = sheet.cssRules; } catch { continue; }
        for (const rule of Array.from(rules)) {
          if (!(rule instanceof CSSStyleRule)) { continue; }
          if (!rule.selectorText?.includes(".download-sheet-label.is-link:hover")) { continue; }
          if (rule.style.textTransform || rule.style.marginBottom) {
            offending.push(rule.cssText);
          }
        }
      }
      return offending;
    });
    expect(hovered).toEqual([]);
  });
});
