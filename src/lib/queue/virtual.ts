// QUE-08: fixed-row-height list virtualisation for the Clip Queue. Pure and
// framework-free so the window maths is unit-tested; the component only binds
// scrollTop / clientHeight and renders rows [start, end).

/** Input of {@link virtualWindow}. All lengths are CSS pixels. */
export interface VirtualWindowInput {
  /** Current scroll offset of the scrolling element. */
  scrollTop: number;
  /** Visible height of the scrolling element. */
  viewportHeight: number;
  /** Fixed height of every row (QUE-08: fixed row height). */
  rowHeight: number;
  /** Number of rows in the (filtered, sorted) list. */
  count: number;
  /** Extra rows rendered above and below the viewport (default 4). */
  overscan?: number;
}

/** The slice of rows to render. */
export interface VirtualWindow {
  /** First rendered row (inclusive). */
  start: number;
  /** One past the last rendered row (exclusive). */
  end: number;
  /** Pixel offset of row `start` from the top of the list. */
  offsetTop: number;
  /** Height of the whole list (the scroll spacer). */
  totalHeight: number;
}

/**
 * Rows to render for a scroll position: only the visible rows plus
 * `overscan` on each side, so 500 clips cost the same as 15 (QUE-08).
 * Scroll offsets outside the list (overscroll, a list that just shrank) are
 * clamped; an unmeasured viewport (height 0) still renders the first rows.
 */
export function virtualWindow(input: VirtualWindowInput): VirtualWindow {
  const count = Number.isFinite(input.count) ? Math.max(0, Math.floor(input.count)) : 0;
  const rowHeight = input.rowHeight > 0 ? input.rowHeight : 1;
  if (count === 0) return { start: 0, end: 0, offsetTop: 0, totalHeight: 0 };
  const totalHeight = count * rowHeight;
  const viewport = Number.isFinite(input.viewportHeight) ? Math.max(0, input.viewportHeight) : 0;
  const maxTop = Math.max(0, totalHeight - viewport);
  const top = Math.min(Math.max(0, Number.isFinite(input.scrollTop) ? input.scrollTop : 0), maxTop);
  const overscan = Math.max(0, Math.floor(input.overscan ?? 4));
  const first = Math.min(count - 1, Math.floor(top / rowHeight));
  const last = Math.max(first + 1, Math.ceil((top + viewport) / rowHeight));
  const start = Math.max(0, first - overscan);
  const end = Math.min(count, last + overscan);
  return { start, end, offsetTop: start * rowHeight, totalHeight };
}

/** Input of {@link revealScrollTop}. */
export interface RevealInput {
  /** Row to reveal; negative = not in the list. */
  index: number;
  rowHeight: number;
  scrollTop: number;
  viewportHeight: number;
}

/**
 * Scroll offset that brings row `index` fully into view with the least
 * movement, or `null` when it is already fully visible (QUE-03: the primary
 * row follows ↑/↓). A row taller than the viewport aligns to its top.
 */
export function revealScrollTop({ index, rowHeight, scrollTop, viewportHeight }: RevealInput): number | null {
  if (!Number.isFinite(index) || index < 0 || rowHeight <= 0) return null;
  const top = Math.floor(index) * rowHeight;
  const bottom = top + rowHeight;
  if (top < scrollTop) return top;
  if (bottom > scrollTop + viewportHeight) return Math.max(0, Math.min(top, bottom - viewportHeight));
  return null;
}
