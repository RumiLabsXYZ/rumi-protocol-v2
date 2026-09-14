import { describe, expect, it } from "vitest";
import indexHtml from "../index.html?raw";
import publicLogo from "../public/brand/rumi.svg?raw";
import sourceLogo from "../../rumi_homepage/static/rumilogo-vector-v2_NEW_inset2.svg?raw";

describe("Rumi favicon", () => {
  it("references the official SVG with browser-compatible metadata", () => {
    expect(indexHtml).toMatch(
      /<link\s+rel="icon"\s+type="image\/svg\+xml"\s+sizes="any"\s+href="\/brand\/rumi\.svg"\s*\/>/,
    );
  });

  it("uses the unchanged official Rumi logo bytes", () => {
    expect(publicLogo).toBe(sourceLogo);
  });
});
