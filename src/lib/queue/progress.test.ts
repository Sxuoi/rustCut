import { describe, expect, it } from "vitest";
import { estimateRemaining, finishedCount, overallFraction, smoothEta, summaryCounts } from "./progress";

describe("EXP-09 ETA", () => {
  it("extrapolates remaining time from the duration-weighted fraction", () => {
    expect(estimateRemaining(0.5, 10)).toBe(10);
    expect(estimateRemaining(0.25, 30)).toBe(90);
    expect(estimateRemaining(1, 30)).toBe(0);
  });

  it("is unknown before 1 % or without elapsed time", () => {
    expect(estimateRemaining(0.005, 10)).toBeNull();
    expect(estimateRemaining(0.5, 0)).toBeNull();
    expect(estimateRemaining(Number.NaN, 10)).toBeNull();
  });

  it("smooths samples exponentially and ignores invalid ones", () => {
    expect(smoothEta(null, 40)).toBe(40);
    expect(smoothEta(40, 20, 0.5)).toBe(30);
    expect(smoothEta(40, 20)).toBeCloseTo(34, 6);
    expect(smoothEta(30, null)).toBe(30);
    expect(smoothEta(30, -1)).toBe(30);
    expect(smoothEta(30, Number.POSITIVE_INFINITY)).toBe(30);
    expect(smoothEta(null, null)).toBeNull();
  });

  it("converges on a steady value", () => {
    let eta: number | null = null;
    for (let i = 0; i < 40; i++) eta = smoothEta(eta, i === 0 ? 100 : 10);
    expect(eta).toBeCloseTo(10, 3);
  });
});

describe("EXP-09/10 counters", () => {
  it("clamps the overall fraction", () => {
    expect(overallFraction({ overall_fraction: 0.5 })).toBe(0.5);
    expect(overallFraction({ overall_fraction: 1.2 })).toBe(1);
    expect(overallFraction({ overall_fraction: -1 })).toBe(0);
    expect(overallFraction(null)).toBe(0);
  });

  it("counts finished clips (done + failed)", () => {
    expect(finishedCount({ done: 3, failed: 1 })).toBe(4);
  });

  it("summarises a batch", () => {
    expect(summaryCounts({ done: ["a", "b"], failed: ["c"], skipped: [], cancelled: ["d"] })).toEqual({
      done: 2,
      failed: 1,
      skipped: 0,
      cancelled: 1,
      total: 4,
    });
  });
});
