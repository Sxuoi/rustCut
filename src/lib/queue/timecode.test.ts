import { describe, expect, it } from "vitest";
import { formatClock, parseTimecode, toMs } from "./timecode";

describe("parseTimecode (mirror of rustcut_model::time::parse_timecode)", () => {
  it("parses SS, MM:SS and H:MM:SS with optional fractions", () => {
    expect(parseTimecode("90")).toBe(90);
    expect(parseTimecode("1:30")).toBe(90);
    expect(parseTimecode(" 3:00 ")).toBe(180);
    expect(parseTimecode("1:02:03.5")).toBe(3723.5);
    expect(parseTimecode("00:15:20.120")).toBe(920.12);
    expect(parseTimecode("1.")).toBe(1);
    expect(parseTimecode(".5")).toBe(0.5);
    expect(parseTimecode("61")).toBe(61);
  });

  it("rounds to milliseconds", () => {
    expect(parseTimecode("0:59.9999")).toBe(60);
    expect(parseTimecode("1.23456")).toBe(1.235);
  });

  it("rejects malformed input", () => {
    for (const bad of ["", "  ", "1:60", "1:2:3:4", "a", "1::2", ".", "1.2.3", "-1", "1:-2", "1:2a", "1:60:00"]) {
      expect(parseTimecode(bad), bad).toBeNull();
    }
  });
});

describe("toMs", () => {
  it("rounds seconds to whole milliseconds", () => {
    expect(toMs(920.12)).toBe(920120);
    expect(toMs(10.0004)).toBe(10000);
    expect(toMs(Number.NaN)).toBe(0);
  });
});

describe("FRM-08 formatClock", () => {
  it("formats platform limits", () => {
    expect(formatClock(180)).toBe("3:00");
    expect(formatClock(600)).toBe("10:00");
    expect(formatClock(3723)).toBe("1:02:03");
    expect(formatClock(59.6)).toBe("1:00");
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(Number.NaN)).toBe("0:00");
  });
});
