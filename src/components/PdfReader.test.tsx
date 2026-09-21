import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "solid-js/web";

/** The document never loads: this file is about what the reader ASKS pdf.js
 *  for, not about what pdf.js draws. */
const pdf = vi.hoisted(() => ({
  getDocument: vi.fn(() => ({ promise: new Promise(() => {}), destroy: vi.fn(async () => {}) })),
}));
vi.mock("pdfjs-dist", () => ({ GlobalWorkerOptions: {}, getDocument: pdf.getDocument }));
vi.mock("pdfjs-dist/build/pdf.worker.mjs?url", () => ({ default: "pdf.worker.mjs" }));

import { PdfReader, documentOptions } from "./PdfReader";

/** What each engine really reports - jsdom's own UA says "linux" in lower case
 *  and would match neither branch. */
const WEBKITGTK = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";
const WKWEBVIEW = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
const WEBVIEW2 = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0";

function withUserAgent(ua: string) {
  Object.defineProperty(window.navigator, "userAgent", { value: ua, configurable: true });
}

describe("PdfReader document options", () => {
  const realUa = window.navigator.userAgent;
  beforeEach(() => pdf.getDocument.mockClear());
  afterEach(() => {
    withUserAgent(realUa);
    document.body.innerHTML = "";
  });

  /** WebKitGTK paints pdf.js's OffscreenCanvas bitmaps into the page canvas
   *  only some of the time: pages come out pure white while pdf.js reports a
   *  finished render and logs nothing. Measured 2026-09-21 on WebKitGTK 2.52.6
   *  over the same nine pages, three runs each - PC World (JPX + JBIG2 mask)
   *  14 of 18 white by default, 0 of 18 with the flag; Computer Gaming World
   *  (plain JPEG) 12 of 12 white, 0 of 12. Poppler renders every one of them. */
  it("keeps the worker off OffscreenCanvas on WebKitGTK", () => {
    expect(documentOptions(WEBKITGTK)).toMatchObject({ isOffscreenCanvasSupported: false });
  });

  /** WKWebView renders these issues correctly on the default path (§19), and a
   *  workaround measured on one engine must not move the others. */
  it("leaves the other engines on pdf.js's default", () => {
    expect(documentOptions(WKWEBVIEW)).not.toHaveProperty("isOffscreenCanvasSupported");
    expect(documentOptions(WEBVIEW2)).not.toHaveProperty("isOffscreenCanvasSupported");
  });

  it("keeps the staged decoders in every case", () => {
    for (const ua of [WEBKITGTK, WKWEBVIEW, WEBVIEW2]) {
      expect(documentOptions(ua)).toMatchObject({ wasmUrl: expect.stringMatching(/pdfjs\/$/) });
    }
  });

  /** The option only helps if the reader actually passes it: the call site is
   *  what a refactor would drop. */
  it("passes the options to getDocument when it opens a document", () => {
    withUserAgent(WEBKITGTK);
    const host = document.createElement("div");
    document.body.appendChild(host);
    const dispose = render(() => <PdfReader src="http://127.0.0.1:1/issue.pdf" />, host);

    expect(pdf.getDocument).toHaveBeenCalledTimes(1);
    expect(pdf.getDocument).toHaveBeenCalledWith(expect.objectContaining({
      url: "http://127.0.0.1:1/issue.pdf",
      isOffscreenCanvasSupported: false,
    }));

    dispose();
    host.remove();
  });
});
