// MRK-01 / MRK-05 mark rules, AC-02 and AC-03 sequences.
import { describe, expect, it } from "vitest";
import {
  clampToSource,
  DEFAULT_MIN_CLIP_SEC,
  formatSeconds,
  markTarget,
  NO_MARKS,
  normalizeMinDuration,
  resolveAdd,
  resolveRetime,
  setIn,
  setMark,
  setOut,
  type PendingMarks,
} from "./marks";

const DUR = 9000; // 2.5 h source

describe("MRK-01 clamping", () => {
  it("clamps marks to [0, duration] at millisecond precision", () => {
    expect(clampToSource(-3, DUR)).toBe(0);
    expect(clampToSource(DUR + 10, DUR)).toBe(DUR);
    expect(clampToSource(12.34567, DUR)).toBe(12.346);
    expect(clampToSource(Number.NaN, DUR)).toBe(0);
  });

  it("never rounds past a non-millisecond duration", () => {
    expect(clampToSource(10.0006, 10.0004)).toBe(10);
  });

  it("only clamps at 0 when the duration is unknown", () => {
    expect(clampToSource(42.5, 0)).toBe(42.5);
    expect(clampToSource(-1, 0)).toBe(0);
  });

  it("setIn / setOut clamp too", () => {
    expect(setIn(NO_MARKS, -5, DUR).marks.in).toBe(0);
    expect(setOut(NO_MARKS, DUR + 99, DUR).marks.out).toBe(DUR);
  });

  it("normalizes the minimum-duration setting to 0.1-5 s", () => {
    expect(normalizeMinDuration(0.01)).toBe(0.1);
    expect(normalizeMinDuration(9)).toBe(5);
    expect(normalizeMinDuration(Number.NaN)).toBe(DEFAULT_MIN_CLIP_SEC);
    expect(normalizeMinDuration(1.5)).toBe(1.5);
  });
});

describe("MRK-01 conflicting marks clear the other one, never swap", () => {
  it("I at or after a pending Out clears the Out with a toast", () => {
    const r = setIn({ in: null, out: 100 }, 120, DUR);
    expect(r.marks).toEqual({ in: 120, out: null });
    expect(r.cleared).toBe("out");
    expect(r.toast).toBe("Out point cleared");
  });

  it("I exactly at the Out conflicts (In >= Out)", () => {
    const r = setIn({ in: null, out: 100 }, 100, DUR);
    expect(r.marks).toEqual({ in: 100, out: null });
    expect(r.cleared).toBe("out");
  });

  it("O at or before a pending In clears the In with a toast", () => {
    const r = setOut({ in: 50, out: null }, 40, DUR);
    expect(r.marks).toEqual({ in: null, out: 40 });
    expect(r.cleared).toBe("in");
    expect(r.toast).toBe("In point cleared");
    expect(setOut({ in: 50, out: null }, 50, DUR).cleared).toBe("in");
  });

  it("non-conflicting marks keep the other mark and show no toast", () => {
    const a = setIn({ in: null, out: 100 }, 90, DUR);
    expect(a.marks).toEqual({ in: 90, out: 100 });
    expect(a.toast).toBeNull();
    const b = setOut({ in: 90, out: 100 }, 95, DUR);
    expect(b.marks).toEqual({ in: 90, out: 95 });
    expect(b.cleared).toBeNull();
  });

  it("re-marking the same side replaces it", () => {
    expect(setMark("in", { in: 10, out: 30 }, 20, DUR).marks).toEqual({ in: 20, out: 30 });
    expect(setMark("out", { in: 10, out: 30 }, 25, DUR).marks).toEqual({ in: 10, out: 25 });
  });
});

describe("MRK-01 add (Enter / A)", () => {
  it("AC-01: I at t1, O at t2, Enter adds [t1, t2]", () => {
    let m: PendingMarks = NO_MARKS;
    m = setIn(m, 920.12, DUR).marks;
    m = setOut(m, 955.8, DUR).marks;
    const r = resolveAdd(m, 999, 0.5, DUR);
    expect(r).toEqual({ ok: true, range: { start: 920.12, end: 955.8 }, outFromPlayhead: false });
  });

  it("AC-02: O, then I at an earlier time, then Enter creates a valid clip", () => {
    let m: PendingMarks = NO_MARKS;
    m = setOut(m, 955.8, DUR).marks;
    m = setIn(m, 920.12, DUR).marks;
    const r = resolveAdd(m, 960, 0.5, DUR);
    expect(r.ok && r.range).toEqual({ start: 920.12, end: 955.8 });
  });

  it("AC-02: I then Enter creates [In, playhead]", () => {
    const m = setIn(NO_MARKS, 10, DUR).marks;
    const r = resolveAdd(m, 25.5, 0.5, DUR);
    expect(r).toEqual({ ok: true, range: { start: 10, end: 25.5 }, outFromPlayhead: true });
  });

  it("Out = playhead is clamped to the source duration", () => {
    const r = resolveAdd({ in: DUR - 2, out: null }, DUR + 5, 0.5, DUR);
    expect(r.ok && r.range).toEqual({ start: DUR - 2, end: DUR });
  });

  it("without an In there is nothing to add (specific error)", () => {
    const none = resolveAdd(NO_MARKS, 10, 0.5, DUR);
    expect(none.ok).toBe(false);
    if (!none.ok) {
      expect(none.error.kind).toBe("no_in");
      expect(none.error.message).toMatch(/In point/);
    }
    expect(resolveAdd({ in: null, out: 50 }, 10, 0.5, DUR).ok).toBe(false);
  });

  it("a playhead at or before In is not a valid Out", () => {
    const before = resolveAdd({ in: 30, out: null }, 20, 0.5, DUR);
    expect(before.ok).toBe(false);
    if (!before.ok) expect(before.error.kind).toBe("out_before_in");
    const same = resolveAdd({ in: 30, out: null }, 30, 0.5, DUR);
    expect(same.ok).toBe(false);
  });

  it("too short: the message states the minimum", () => {
    const r = resolveAdd({ in: 10, out: 10.3 }, 99, 0.5, DUR);
    expect(r.ok).toBe(false);
    if (!r.ok) {
      expect(r.error.kind).toBe("too_short");
      expect(r.error.message).toContain("0.3 s");
      expect(r.error.message).toContain("0.5 s");
    }
    const custom = resolveAdd({ in: 10, out: 11 }, 99, 2, DUR);
    expect(!custom.ok && custom.error.message).toContain("2 s");
  });

  it("exactly the minimum duration is allowed (float-safe)", () => {
    expect(resolveAdd({ in: 1.1, out: 1.6 }, 0, 0.5, DUR).ok).toBe(true);
    expect(resolveAdd({ in: 0.3, out: 0.8 }, 0, 0.5, DUR).ok).toBe(true);
    expect(resolveAdd({ in: 0.7, out: 0.8 }, 0, 0.1, DUR).ok).toBe(true);
  });

  it("the minimum setting is clamped to its 0.1-5 s range", () => {
    // A 0.05 s setting behaves as 0.1 s.
    expect(resolveAdd({ in: 1, out: 1.08 }, 0, 0.05, DUR).ok).toBe(false);
    expect(resolveAdd({ in: 1, out: 1.1 }, 0, 0.05, DUR).ok).toBe(true);
  });
});

describe("AC-03: no input sequence creates a clip with Out <= In", () => {
  // Deterministic PRNG (mulberry32).
  function rng(seed: number): () => number {
    let a = seed >>> 0;
    return () => {
      a = (a + 0x6d2b79f5) >>> 0;
      let x = Math.imul(a ^ (a >>> 15), 1 | a);
      x = (x + Math.imul(x ^ (x >>> 7), 61 | x)) ^ x;
      return ((x ^ (x >>> 14)) >>> 0) / 4294967296;
    };
  }

  it("holds over 20,000 random I / O / Enter sequences", () => {
    const rand = rng(42);
    const duration = 120;
    let m: PendingMarks = NO_MARKS;
    let added = 0;
    for (let step = 0; step < 20_000; step++) {
      const min = [0.1, 0.5, 1, 5][Math.floor(rand() * 4)];
      // Times include out-of-range values and near-duplicates.
      const time = rand() < 0.1 ? (rand() - 0.5) * 400 : Math.round(rand() * duration * 1000) / 1000;
      const op = rand();
      if (op < 0.35) {
        const r = setIn(m, time, duration);
        if (r.cleared) expect(r.toast).toBe("Out point cleared");
        m = r.marks;
      } else if (op < 0.7) {
        const r = setOut(m, time, duration);
        if (r.cleared) expect(r.toast).toBe("In point cleared");
        m = r.marks;
      } else {
        const r = resolveAdd(m, time, min, duration);
        if (r.ok) {
          added++;
          expect(r.range.end).toBeGreaterThan(r.range.start);
          expect(r.range.end - r.range.start).toBeGreaterThanOrEqual(min - 1e-6);
          expect(r.range.start).toBeGreaterThanOrEqual(0);
          expect(r.range.end).toBeLessThanOrEqual(duration);
          m = NO_MARKS; // the caller clears after adding
        }
      }
      // Pending marks themselves are never inverted (never swapped).
      if (m.in !== null && m.out !== null) expect(m.out).toBeGreaterThan(m.in);
    }
    expect(added).toBeGreaterThan(100);
  });
});

describe("MRK-05 keyboard retime (Alt+I / Alt+O)", () => {
  const clip = { start_sec: 10, end_sec: 20 };

  it("moves one edge to the playhead", () => {
    expect(resolveRetime(clip, "in", 12, 0.5, DUR)).toEqual({ ok: true, range: { start: 12, end: 20 } });
    expect(resolveRetime(clip, "out", 30, 0.5, DUR)).toEqual({ ok: true, range: { start: 10, end: 30 } });
  });

  it("rejects an In past the Out and an Out before the In", () => {
    const a = resolveRetime(clip, "in", 25, 0.5, DUR);
    expect(!a.ok && a.error.kind).toBe("out_before_in");
    const b = resolveRetime(clip, "out", 5, 0.5, DUR);
    expect(!b.ok && b.error.kind).toBe("out_before_in");
  });

  it("rejects a result shorter than the minimum", () => {
    const r = resolveRetime(clip, "in", 19.8, 0.5, DUR);
    expect(!r.ok && r.error.kind).toBe("too_short");
  });
});

describe("Shift+I / Shift+O targets", () => {
  it("prefers the pending mark, else the selected clip, else nothing", () => {
    const sel = { start_sec: 5, end_sec: 9 };
    expect(markTarget("in", { in: 3, out: null }, sel)).toBe(3);
    expect(markTarget("out", { in: 3, out: null }, sel)).toBe(9);
    expect(markTarget("in", NO_MARKS, sel)).toBe(5);
    expect(markTarget("out", NO_MARKS, null)).toBeNull();
  });
});

describe("formatSeconds", () => {
  it("drops trailing zeros", () => {
    expect(formatSeconds(0.5)).toBe("0.5");
    expect(formatSeconds(2)).toBe("2");
    expect(formatSeconds(0.25)).toBe("0.25");
    expect(formatSeconds(Number.POSITIVE_INFINITY)).toBe("0");
  });
});
