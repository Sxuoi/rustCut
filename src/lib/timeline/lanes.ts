// Lane assignment for overlapping clips (MRK-02, MRK-03). Pure, no DOM.

/** A time range in source seconds. */
export interface Span {
  start: number;
  end: number;
}

/** MRK-02: up to 4 lanes are visible, the rest collapse into "+N" badges. */
export const MAX_VISIBLE_LANES = 4;

/** A run of overlapping hidden clips, summarised as one "+N" badge. */
export interface OverflowRegion {
  start: number;
  end: number;
  count: number;
}

export interface LaneLayout {
  /** Lane of each input span (input order). Values >= `maxLanes` are hidden. */
  lane: Int16Array;
  /** Number of visible lanes actually used (0..maxLanes). */
  laneCount: number;
  /** Hidden clips grouped per overlapping region. */
  overflow: OverflowRegion[];
}

const EPS = 1e-9;

/**
 * Greedy interval partitioning: spans are visited by start (ties by input
 * order, so the result is stable) and put into the first lane that is free,
 * i.e. whose last span ended at or before this start. Touching spans share a
 * lane. Lanes beyond `maxLanes` are hidden and counted per region.
 */
export function assignLanes(spans: readonly Span[], maxLanes: number = MAX_VISIBLE_LANES): LaneLayout {
  const n = spans.length;
  const order = new Int32Array(n);
  for (let i = 0; i < n; i++) order[i] = i;
  order.sort((a, b) => spans[a].start - spans[b].start || a - b);

  const lane = new Int16Array(n);
  const laneEnds: number[] = [];
  for (let k = 0; k < n; k++) {
    const i = order[k];
    const s = spans[i];
    let l = 0;
    while (l < laneEnds.length && laneEnds[l] > s.start + EPS) l++;
    if (l === laneEnds.length) laneEnds.push(s.end);
    else laneEnds[l] = s.end;
    lane[i] = l;
  }

  const overflow: OverflowRegion[] = [];
  let cur: OverflowRegion | null = null;
  for (let k = 0; k < n; k++) {
    const i = order[k];
    if (lane[i] < maxLanes) continue;
    const s = spans[i];
    if (cur && s.start < cur.end - EPS) {
      cur.count++;
      if (s.end > cur.end) cur.end = s.end;
    } else {
      cur = { start: s.start, end: s.end, count: 1 };
      overflow.push(cur);
    }
  }
  return { lane, laneCount: Math.min(maxLanes, laneEnds.length), overflow };
}

/**
 * MRK-03: 1 for every clip whose range (to the millisecond) equals another
 * clip's range, else 0.
 */
export function duplicateFlags(spans: readonly Span[]): Uint8Array {
  const flags = new Uint8Array(spans.length);
  const seen = new Map<string, number>();
  for (let i = 0; i < spans.length; i++) {
    const key = `${Math.round(spans[i].start * 1000)}:${Math.round(spans[i].end * 1000)}`;
    const first = seen.get(key);
    if (first === undefined) seen.set(key, i);
    else {
      flags[first] = 1;
      flags[i] = 1;
    }
  }
  return flags;
}
