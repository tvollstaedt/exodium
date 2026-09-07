import { describe, it, expect } from "vitest";
import { matchesLibraryQuery, platformTag } from "./util";

describe("matchesLibraryQuery", () => {
  const game = { title: "Magic Carpet Plus", sort_title: "Magic Carpet Plus" };

  it("matches case-insensitively on a substring", () => {
    expect(matchesLibraryQuery(game, "carpet")).toBe(true);
    expect(matchesLibraryQuery(game, "CARPET")).toBe(true);
    expect(matchesLibraryQuery(game, "magic car")).toBe(true);
  });

  it("rejects non-matches", () => {
    expect(matchesLibraryQuery(game, "descent")).toBe(false);
  });

  // An empty or whitespace-only box must show the full library, not nothing.
  it("keeps everything for a blank query", () => {
    expect(matchesLibraryQuery(game, "")).toBe(true);
    expect(matchesLibraryQuery(game, "   ")).toBe(true);
  });

  it("matches on sort_title when the display title differs", () => {
    const withArticle = { title: "The Legend of Kyrandia", sort_title: "Legend of Kyrandia, The" };
    expect(matchesLibraryQuery(withArticle, "legend of kyrandia, the")).toBe(true);
  });

  it("tolerates missing titles", () => {
    expect(matchesLibraryQuery({}, "anything")).toBe(false);
    expect(matchesLibraryQuery({ title: null, sort_title: null }, "x")).toBe(false);
  });
});

describe("matchesLibraryQuery across language variants", () => {
  // Merged cards show the English title, so without variant_titles someone
  // searching the German name found nothing in My Library while Browse (which
  // filters in SQL across every variant) found it.
  const merged = {
    title: "The Office",
    sort_title: "Office, The",
    variant_titles: "Das Amt\u001fEl Despacho",
  };

  it("matches a localized title carried by another variant", () => {
    expect(matchesLibraryQuery(merged, "Das Amt")).toBe(true);
    expect(matchesLibraryQuery(merged, "despacho")).toBe(true);
  });

  it("still matches the card's own title", () => {
    expect(matchesLibraryQuery(merged, "office")).toBe(true);
  });

  it("does not match an unrelated query", () => {
    expect(matchesLibraryQuery(merged, "kyrandia")).toBe(false);
  });
});

describe("platformTag", () => {
  it("names the collection family, and nothing for unknown sources", () => {
    expect(platformTag("eXoDOS")).toBe("DOS");
    expect(platformTag("eXoDOS_GLP")).toBe("DOS");
    expect(platformTag("eXoWin3x")).toBe("Win3x");
    expect(platformTag("eXoWin9x")).toBe("Win9x");
    expect(platformTag("eXoScummVM")).toBe("ScummVM");
    expect(platformTag(null)).toBeNull();
    expect(platformTag("something")).toBeNull();
  });
});
