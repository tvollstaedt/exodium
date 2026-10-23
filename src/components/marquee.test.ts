import { describe, it, expect } from "vitest";
import { intersecting } from "./marquee";

describe("marquee", () => {
  it("takes every card the rectangle touches, nothing beside it", () => {
    const cards = [
      { id: 1, box: { left: 0, top: 0, right: 100, bottom: 140 } },
      { id: 2, box: { left: 110, top: 0, right: 210, bottom: 140 } },
      { id: 3, box: { left: 0, top: 150, right: 100, bottom: 290 } },
    ];
    expect(intersecting(cards, { left: 90, top: 100, right: 120, bottom: 160 })).toEqual([1, 2, 3]);
    expect(intersecting(cards, { left: 101, top: 0, right: 109, bottom: 300 })).toEqual([]);
    expect(intersecting(cards, { left: 120, top: 10, right: 130, bottom: 20 })).toEqual([2]);
  });
});
