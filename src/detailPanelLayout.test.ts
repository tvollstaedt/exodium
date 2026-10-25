import { describe, it, expect } from "vitest";
import {
  clampPanelWidth, fractionFromPointer, parseWidthFraction, serializeWidthFraction, snapFraction,
  PANEL_MIN_PX, GRID_MIN_PX,
} from "./detailPanelLayout";

describe("detail panel width", () => {
  it("parses only a fraction strictly between 0 and 1", () => {
    expect(parseWidthFraction(null)).toBeNull();
    expect(parseWidthFraction("")).toBeNull();
    expect(parseWidthFraction("abc")).toBeNull();
    expect(parseWidthFraction("0")).toBeNull();
    expect(parseWidthFraction("1")).toBeNull();
    expect(parseWidthFraction("1.5")).toBeNull();
    expect(parseWidthFraction("0.5")).toBe(0.5);
  });

  it("round-trips through the config string, null as the empty string", () => {
    expect(serializeWidthFraction(null)).toBe("");
    expect(parseWidthFraction(serializeWidthFraction(0.3333))).toBeCloseTo(0.3333, 4);
  });

  it("snaps to thirds and the half within the tolerance only", () => {
    expect(snapFraction(0.51)).toBe(0.5);
    expect(snapFraction(0.49)).toBe(0.5);
    expect(snapFraction(0.34)).toBe(1 / 3);
    expect(snapFraction(0.66)).toBe(2 / 3);
    expect(snapFraction(0.55)).toBe(0.55);
    expect(snapFraction(0.42)).toBe(0.42);
  });

  it("clamps to the minimum and to the window minus the grid margin", () => {
    expect(clampPanelWidth(0.5, 1600)).toBe(800);
    expect(clampPanelWidth(0.1, 1600)).toBe(PANEL_MIN_PX);
    expect(clampPanelWidth(0.99, 1600)).toBe(1600 - GRID_MIN_PX);
    // A window too small for both: the panel's minimum wins over the margin.
    expect(clampPanelWidth(0.9, 500)).toBe(PANEL_MIN_PX);
  });

  it("reads the pointer from the right edge", () => {
    expect(fractionFromPointer(1200, 1600)).toBeCloseTo(0.25);
    expect(fractionFromPointer(-50, 1600)).toBe(1);
    expect(fractionFromPointer(2000, 1600)).toBe(0);
    expect(fractionFromPointer(100, 0)).toBe(0);
  });
});
