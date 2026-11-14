// Canvas plumbing shared by both timelines: DPR scaling, rAF scheduling,
// theme tokens read from CSS custom properties.

/** Size the backing store for devicePixelRatio; returns a ctx in CSS px. */
export function sizeCanvas(canvas: HTMLCanvasElement, width: number, height: number): CanvasRenderingContext2D | null {
  const dpr = typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1;
  const pw = Math.max(1, Math.round(width * dpr));
  const ph = Math.max(1, Math.round(height * dpr));
  if (canvas.width !== pw) canvas.width = pw;
  if (canvas.height !== ph) canvas.height = ph;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  ctx.setTransform(pw / Math.max(1, width), 0, 0, ph / Math.max(1, height), 0, 0);
  return ctx;
}

/** Coalesces many `request()` calls into one `draw` per animation frame. */
export class FrameScheduler {
  private handle: number | null = null;
  constructor(private readonly draw: () => void) {}

  request(): void {
    if (this.handle !== null) return;
    this.handle = requestAnimationFrame(() => {
      this.handle = null;
      this.draw();
    });
  }

  cancel(): void {
    if (this.handle !== null) cancelAnimationFrame(this.handle);
    this.handle = null;
  }
}

export interface Theme {
  bg: string;
  panel: string;
  line: string;
  text: string;
  muted: string;
  accent: string;
  pendingIn: string;
  pendingOut: string;
  playhead: string;
  success: string;
}

const FALLBACK: Theme = {
  bg: "#0d1014",
  panel: "#141a21",
  line: "#2c3745",
  text: "#e6edf3",
  muted: "#9aa8b6",
  accent: "#5aa4ff",
  pendingIn: "#fbbf24",
  pendingOut: "#fb923c",
  playhead: "#f8fafc",
  success: "#4ade80",
};

/** Read the design tokens (app.css) from the document root. */
export function readTheme(): Theme {
  if (typeof document === "undefined") return FALLBACK;
  const cs = getComputedStyle(document.documentElement);
  const v = (name: string, fb: string) => cs.getPropertyValue(name).trim() || fb;
  return {
    bg: v("--color-bg", FALLBACK.bg),
    panel: v("--color-panel", FALLBACK.panel),
    line: v("--color-line", FALLBACK.line),
    text: v("--color-text", FALLBACK.text),
    muted: v("--color-muted", FALLBACK.muted),
    accent: v("--color-accent", FALLBACK.accent),
    pendingIn: v("--color-pending-in", FALLBACK.pendingIn),
    pendingOut: v("--color-pending-out", FALLBACK.pendingOut),
    playhead: v("--color-playhead", FALLBACK.playhead),
    success: v("--color-success", FALLBACK.success),
  };
}

/** Diagonal-stripe pattern for the pending range. */
export function makeHatch(ctx: CanvasRenderingContext2D, color: string): CanvasPattern | null {
  if (typeof document === "undefined") return null;
  const c = document.createElement("canvas");
  c.width = 8;
  c.height = 8;
  const g = c.getContext("2d");
  if (!g) return null;
  g.strokeStyle = color;
  g.lineWidth = 1.5;
  g.beginPath();
  g.moveTo(-2, 10);
  g.lineTo(10, -2);
  g.moveTo(-2, 2);
  g.lineTo(2, -2);
  g.moveTo(6, 10);
  g.lineTo(10, 6);
  g.stroke();
  return ctx.createPattern(c, "repeat");
}
