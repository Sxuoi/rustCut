// Placement of the queue's popovers (tag picker, failure details) and focus
// helpers shared by the queue, export and timestamp components.

/** A DOMRect-like box in viewport coordinates. */
export interface Box {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** Where a popover goes. */
export interface Placement {
  left: number;
  top: number;
  side: "below" | "above";
}

/**
 * Put a `size` popover below its anchor when it fits, else above, else on
 * the side with more room; always clamped inside the viewport margins.
 */
export function placePopover(
  anchor: Box,
  size: { width: number; height: number },
  viewport: { width: number; height: number },
  gap = 4,
  margin = 8,
): Placement {
  const below = anchor.bottom + gap;
  const above = anchor.top - gap - size.height;
  const fitsBelow = below + size.height <= viewport.height - margin;
  const fitsAbove = above >= margin;
  let side: Placement["side"];
  if (fitsBelow) side = "below";
  else if (fitsAbove) side = "above";
  else side = viewport.height - anchor.bottom >= anchor.top ? "below" : "above";
  const rawTop = side === "below" ? below : above;
  const maxTop = Math.max(margin, viewport.height - margin - size.height);
  const top = Math.min(Math.max(margin, rawTop), maxTop);
  const maxLeft = Math.max(margin, viewport.width - margin - size.width);
  const left = Math.min(Math.max(margin, anchor.left), maxLeft);
  return { left, top, side };
}

/** The element focus returns to after queue/dialog interactions (fe-shell convention). */
export const FOCUS_HOME_SELECTOR = '[data-focus-home="player"]';

/**
 * Give keyboard focus back to the player so global shortcuts work again.
 * Without a focus home the active element is blurred (focus goes to body).
 */
export function focusHome(doc: Document | undefined = typeof document === "undefined" ? undefined : document): void {
  if (!doc) return;
  const home = doc.querySelector<HTMLElement>(FOCUS_HOME_SELECTOR);
  if (home) {
    home.focus({ preventScroll: true });
    return;
  }
  const active = doc.activeElement;
  if (active && "blur" in active && typeof active.blur === "function") (active as HTMLElement).blur();
}

/**
 * focusHome() after an ASYNC queue action, unless a modal dialog took focus meanwhile: Ctrl+E
 * pressed while an include edit was in flight opens the export dialog, and pulling focus back
 * to the player would make the next Enter add a clip instead of starting the export.
 */
export function focusHomeUnlessModal(doc: Document | undefined = typeof document === "undefined" ? undefined : document): void {
  if (!doc) return;
  const active = doc.activeElement as (Element & { closest?: Element["closest"] }) | null;
  if (active?.closest?.('[aria-modal="true"]')) return;
  focusHome(doc);
}

/** Selector of keyboard-focusable descendants (focus traps, initial focus). */
export const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** Focusable, visible descendants of `root` in DOM order. */
export function focusableIn(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) => !el.hasAttribute("inert") && el.getClientRects().length > 0,
  );
}

/** True when keyboard input goes to a text field (global shortcuts are suspended there). */
export function isTextEntry(el: EventTarget | null): boolean {
  if (!el || typeof (el as HTMLElement).tagName !== "string") return false;
  const node = el as HTMLElement;
  if (node.isContentEditable) return true;
  const tag = node.tagName;
  if (tag === "TEXTAREA") return true;
  if (tag !== "INPUT") return false;
  const type = (node as HTMLInputElement).type;
  return !["checkbox", "radio", "button", "submit", "reset", "range", "color", "file"].includes(type);
}
