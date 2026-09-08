import { describe, it, expect } from "vitest";
import { coverRect, containRect } from "./videoCanvas";

describe("videoCanvas fit math", () => {
  it("cover crops the source and fills the canvas", () => {
    // 16:9 video into a wider 2:1 canvas: full width, vertical crop.
    const r = coverRect(1600, 900, 800, 400);
    expect(r.dw).toBe(800);
    expect(r.dh).toBe(400);
    expect(r.sw).toBe(1600);
    expect(r.sh).toBe(800);
    expect(r.sx).toBe(0);
    expect(r.sy).toBe(50);
  });

  it("contain letterboxes inside the canvas", () => {
    // 4:3 video into a 2:1 canvas: full height, horizontal bars.
    const r = containRect(400, 300, 800, 400);
    expect(r.sw).toBe(400);
    expect(r.sh).toBe(300);
    expect(r.dh).toBe(400);
    expect(r.dw).toBeCloseTo(533.33, 1);
    expect(r.dx).toBeCloseTo((800 - r.dw) / 2, 5);
    expect(r.dy).toBe(0);
  });

  it("a matching aspect is identical in both modes", () => {
    const cover = coverRect(320, 240, 640, 480);
    const contain = containRect(320, 240, 640, 480);
    expect(cover).toEqual({ sx: 0, sy: 0, sw: 320, sh: 240, dx: 0, dy: 0, dw: 640, dh: 480 });
    expect(contain).toEqual({ sx: 0, sy: 0, sw: 320, sh: 240, dx: 0, dy: 0, dw: 640, dh: 480 });
  });
});
