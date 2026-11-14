import { describe, expect, it } from "vitest";
import { formatBytes, formatLeadIn, formatPercent, formatSeconds, indexWidth, tp } from "./format";

const MB = 1024 * 1024;
const GB = 1024 * MB;

describe("QUE-01/EXP-05 indexWidth", () => {
  it("pads to max(2, digits of the highest clip number)", () => {
    expect(indexWidth([])).toBe(2);
    expect(indexWidth([1, 7])).toBe(2);
    expect(indexWidth([3, 99])).toBe(2);
    expect(indexWidth([5, 100])).toBe(3);
    expect(indexWidth([1234, 2])).toBe(4);
  });
});

describe("EXP-11 formatBytes", () => {
  it("shows the panel estimate as ~90 MB", () => {
    expect(formatBytes(90 * MB, true)).toBe("~90 MB");
  });

  it("uses one decimal below 10 and whole numbers above", () => {
    expect(formatBytes(500)).toBe("500 B");
    expect(formatBytes(1536)).toBe("1.5 KB");
    expect(formatBytes(1.2 * GB)).toBe("1.2 GB");
    expect(formatBytes(412 * GB)).toBe("412 GB");
    expect(formatBytes(2 * GB)).toBe("2 GB");
  });

  it("rolls 1023.7 MB over to 1 GB", () => {
    expect(formatBytes(1023.7 * MB)).toBe("1 GB");
  });

  it("shows invalid input as a dash", () => {
    expect(formatBytes(-1)).toBe("—");
    expect(formatBytes(Number.NaN, true)).toBe("—");
  });
});

describe("EXP-09 formatSeconds", () => {
  it("formats seconds, minutes and hours", () => {
    expect(formatSeconds(4)).toBe("4 s");
    expect(formatSeconds(4, true)).toBe("~4 s");
    expect(formatSeconds(0.3)).toBe("1 s");
    expect(formatSeconds(75)).toBe("1 min 15 s");
    expect(formatSeconds(120)).toBe("2 min");
    expect(formatSeconds(600)).toBe("10 min");
    expect(formatSeconds(3599)).toBe("1 h");
    expect(formatSeconds(3900)).toBe("1 h 5 min");
    expect(formatSeconds(7200)).toBe("2 h");
  });

  it("shows invalid input as a dash", () => {
    expect(formatSeconds(Number.NaN)).toBe("—");
    expect(formatSeconds(-5)).toBe("—");
  });
});

describe("EXP-09 formatPercent", () => {
  it("rounds down so 100% only shows when finished", () => {
    expect(formatPercent(0.5)).toBe("50%");
    expect(formatPercent(0.999)).toBe("99%");
    expect(formatPercent(1)).toBe("100%");
  });

  it("clamps out-of-range values", () => {
    expect(formatPercent(1.5)).toBe("100%");
    expect(formatPercent(-1)).toBe("0%");
    expect(formatPercent(Number.NaN)).toBe("0%");
  });
});

describe("MRK-07 formatLeadIn", () => {
  it("shows one decimal with a plus sign", () => {
    expect(formatLeadIn(1.44)).toBe("+1.4 s");
    expect(formatLeadIn(4.25)).toBe("+4.3 s");
    expect(formatLeadIn(0)).toBe("+0.0 s");
    expect(formatLeadIn(-2)).toBe("+0.0 s");
  });
});

describe("§8.5 plural-aware tp()", () => {
  it("picks .one / .other with {count}", () => {
    expect(tp("queue.count.clips", 1)).toBe("1 clip");
    expect(tp("queue.count.clips", 0)).toBe("0 clips");
    expect(tp("queue.count.clips", 3)).toBe("3 clips");
    expect(tp("queue.deleted", 2, { undo: "Ctrl+Z" })).toBe("Deleted 2 clips. Undo with Ctrl+Z.");
  });

  it("falls back to .other when a category is missing", () => {
    expect(tp("missing.key", 1)).toBe("missing.key.other");
  });
});
