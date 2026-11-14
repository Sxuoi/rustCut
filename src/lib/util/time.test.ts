import { describe, expect, it } from "vitest";
import { formatDuration, formatIndex, formatRuler, formatTimecode, roundMs } from "./time";

describe("time formatting (mirrors rustcut-model::time)", () => {
  it("timecode", () => {
    expect(formatTimecode(920.12)).toBe("00:15:20.120");
    expect(formatTimecode(9000)).toBe("02:30:00.000");
    expect(formatTimecode(-3)).toBe("00:00:00.000");
    expect(formatTimecode(59.9996)).toBe("00:01:00.000");
  });

  it("durations", () => {
    expect(formatDuration(49.5)).toBe("0:49.5");
    expect(formatDuration(955.8 - 920.12)).toBe("0:35.7");
    expect(formatDuration(90)).toBe("1:30.0");
  });

  it("ruler, index, rounding", () => {
    expect(formatRuler(3723)).toBe("1:02:03");
    expect(formatRuler(75)).toBe("1:15");
    expect(formatIndex(2)).toBe("#02");
    expect(formatIndex(7, 3)).toBe("#007");
    expect(roundMs(1.23456)).toBe(1.235);
  });
});
