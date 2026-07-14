import { describe, expect, test } from "bun:test";

import { getReadableTextColor } from "../src/utils/colorContrast";

describe("getReadableTextColor", () => {
  test("uses dark text on light avatar colors", () => {
    expect(getReadableTextColor("#f2d66a")).toBe("#000000");
    expect(getReadableTextColor("#fff")).toBe("#000000");
  });

  test("uses white text on dark avatar colors", () => {
    expect(getReadableTextColor("#27364a")).toBe("#ffffff");
    expect(getReadableTextColor("#000000")).toBe("#ffffff");
  });

  test("falls back to white for unsupported color strings", () => {
    expect(getReadableTextColor("var(--avatar-color)")).toBe("#ffffff");
  });
});
