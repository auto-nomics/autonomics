/**
 * Aesthetics rule engine: measures a mounted slide's real DOM geometry
 * and reports findings to the exporter (Rust diagnostics worker) via
 * `window.__deckDiagnose()`. Runs on the print page — the same single
 * render path as GUI preview and PDF/PNG/PPTX export — so findings match
 * shipped pixels exactly.
 *
 * The engine is a pure function over a root element, so tests can mount
 * any SlideView and call `diagnoseSlide` directly.
 */

export interface SlideFinding {
  rule: string;
  severity: "error" | "warning" | "info";
  message: string;
  details?: Record<string, unknown>;
}

export interface DiagnoseOptions {
  slideNo?: number;
}

/** Tolerance (CSS px) before an edge crossing counts as overflow. */
const EDGE_TOLERANCE = 2;

/**
 * Diagnose the slide mounted under `root` (the `.deck-slide` element).
 * All measurements are CSS px; the caller renders at scale 1.0 so they
 * map 1:1 onto canvas coordinates.
 */
export function diagnoseSlide(root: HTMLElement, options: DiagnoseOptions = {}): SlideFinding[] {
  const findings: SlideFinding[] = [];
  const slideNo = options.slideNo;
  const page = slideNo === undefined ? "Slide" : `Slide ${slideNo}`;
  const rect = root.getBoundingClientRect();
  const canvasWidth = rect.width;
  const canvasHeight = rect.height;

  const elements = visibleElements(root);
  const textHolders = elements.filter(hasDirectText);
  const images = Array.from(root.querySelectorAll("img"));

  // ---- empty-slide -----------------------------------------------------
  const totalText = textHolders.reduce((sum, el) => sum + (el.textContent?.length ?? 0), 0);
  if (totalText === 0 && images.length === 0) {
    findings.push({
      rule: "empty-slide",
      severity: "info",
      message: `${page} has no text or image content.`,
    });
    return findings;
  }

  // ---- text-overflow-canvas / font-too-small / low-contrast -------------
  const fontFloor = Math.max(10, Math.round((canvasWidth * 14) / 1280));
  for (const el of textHolders) {
    // A block element's border box hides nowrap ink overflow; a Range
    // over the contents reports the text's true rendered extent.
    const box = textRect(el);
    if (box.width === 0 && box.height === 0) continue;
    const text = snippet(el.textContent ?? "");

    const overflow = describeOverflow(box, rect);
    if (overflow) {
      findings.push({
        rule: "text-overflow-canvas",
        severity: "error",
        message: `${page} text overflows the canvas ${overflow}: "${text}". Trim the text, reduce the font size, or split the slide.`,
        details: { overflow, text, fontSize: getComputedStyle(el).fontSize, ...boundDetails(box, rect) },
      });
    }

    const fontSize = parseFloat(getComputedStyle(el).fontSize);
    if (Number.isFinite(fontSize) && fontSize < fontFloor) {
      findings.push({
        rule: "font-too-small",
        severity: "warning",
        message: `${page} font size ${fontSize}px is below the readability floor ${fontFloor}px: "${text}".`,
        details: { fontSize, floor: fontFloor, text },
      });
    }

    const contrast = contrastRatio(el, root);
    if (contrast !== null && contrast.confident && contrast.ratio < 3) {
      findings.push({
        rule: "low-contrast",
        severity: "warning",
        message: `${page} text/background contrast is only ${contrast.ratio.toFixed(1)}:1 (aim for at least 3:1): "${text}".`,
        details: { ratio: Number(contrast.ratio.toFixed(2)), foreground: contrast.foreground, background: contrast.background, text },
      });
    }
  }

  // ---- image-overflow-canvas -------------------------------------------
  for (const img of images) {
    const box = img.getBoundingClientRect();
    if (box.width === 0 || box.height === 0) continue;
    const overflow = describeOverflow(box, rect);
    if (overflow) {
      findings.push({
        rule: "image-overflow-canvas",
        severity: "error",
        message: `${page} image overflows the canvas ${overflow} (${img.getAttribute("src")?.slice(0, 60) ?? "img"}).`,
        details: { overflow, ...boundDetails(box, rect) },
      });
    }
  }

  // ---- content-clipped ---------------------------------------------------
  for (const el of [...textHolders, ...images] as Element[]) {
    const style = getComputedStyle(el);
    if (style.overflowX === "visible" && style.overflowY === "visible") continue;
    const clippedX = el.scrollWidth - el.clientWidth;
    const clippedY = el.scrollHeight - el.clientHeight;
    if (clippedX > EDGE_TOLERANCE || clippedY > EDGE_TOLERANCE) {
      findings.push({
        rule: "content-clipped",
        severity: "warning",
        message: `${page} content is clipped by its container (horizontal ${clippedX}px / vertical ${clippedY}px): "${snippet(el.textContent ?? "")}".`,
        details: { clippedX, clippedY },
      });
    }
  }

  // ---- text-density ------------------------------------------------------
  const densityLimit = Math.round(1200 * ((canvasWidth * canvasHeight) / (1280 * 720)));
  if (totalText > densityLimit) {
    findings.push({
      rule: "text-density",
      severity: "warning",
      message: `${page} carries about ${totalText} characters of visible text, above the suggested limit of ${densityLimit}; split it across slides.`,
      details: { totalText, limit: densityLimit },
    });
  }

  return findings;
}

// -----------------------------------------------------------------------------
// helpers
// -----------------------------------------------------------------------------

/** Union rect of an element's rendered text (includes ink overflow). */
function textRect(el: Element): DOMRect {
  const range = document.createRange();
  range.selectNodeContents(el);
  return range.getBoundingClientRect();
}

function visibleElements(root: HTMLElement): HTMLElement[] {
  const out: HTMLElement[] = [];
  const walk = (node: Element) => {
    const style = getComputedStyle(node);
    if (style.display === "none" || style.visibility === "hidden" || style.opacity === "0") return;
    if (node.classList.contains("deck-click-hidden")) return;
    out.push(node as HTMLElement);
    for (const child of Array.from(node.children)) walk(child);
  };
  for (const child of Array.from(root.children)) walk(child);
  return out;
}

function hasDirectText(el: Element): boolean {
  for (const node of Array.from(el.childNodes)) {
    if (node.nodeType === Node.TEXT_NODE && (node.textContent ?? "").trim().length > 0) return true;
  }
  return false;
}

function snippet(text: string): string {
  const clean = text.replace(/\s+/g, " ").trim();
  return clean.length > 40 ? `${clean.slice(0, 40)}…` : clean;
}

function describeOverflow(box: DOMRect, canvas: DOMRect): string | null {
  const parts: string[] = [];
  if (box.right > canvas.right + EDGE_TOLERANCE) parts.push(`right edge by ${Math.round(box.right - canvas.right)}px`);
  if (box.bottom > canvas.bottom + EDGE_TOLERANCE) parts.push(`bottom edge by ${Math.round(box.bottom - canvas.bottom)}px`);
  if (box.left < canvas.left - EDGE_TOLERANCE) parts.push(`left edge by ${Math.round(canvas.left - box.left)}px`);
  if (box.top < canvas.top - EDGE_TOLERANCE) parts.push(`top edge by ${Math.round(canvas.top - box.top)}px`);
  return parts.length ? parts.join(", ") : null;
}

function boundDetails(box: DOMRect, canvas: DOMRect): Record<string, number> {
  return {
    left: Math.round(box.left - canvas.left),
    top: Math.round(box.top - canvas.top),
    width: Math.round(box.width),
    height: Math.round(box.height),
  };
}

interface ContrastResult {
  ratio: number;
  foreground: string;
  background: string;
  /** False when the effective background is an image/gradient (ratio is a lower-bound guess at best). */
  confident: boolean;
}

/** WCAG-style contrast ratio of a text element against its nearest opaque ancestor background. */
function contrastRatio(el: Element, root: HTMLElement): ContrastResult | null {
  const fg = parseColor(getComputedStyle(el).color);
  if (!fg) return null;
  let node: Element | null = el;
  while (node && node !== root.parentElement) {
    const style = getComputedStyle(node);
    if (style.backgroundImage !== "none") {
      // Layered over an image: cannot compute honestly.
      return { ratio: Number.POSITIVE_INFINITY, foreground: rgbString(fg), background: "image", confident: false };
    }
    const bg = parseColor(style.backgroundColor);
    if (bg && bg.a >= 0.95) {
      return { ratio: luminanceRatio(fg, bg), foreground: rgbString(fg), background: rgbString(bg), confident: true };
    }
    node = node.parentElement;
  }
  // Fell through to the page: assume the slide's own background.
  const rootBg = parseColor(getComputedStyle(root).backgroundColor);
  if (!rootBg || rootBg.a < 0.95) return null;
  return { ratio: luminanceRatio(fg, rootBg), foreground: rgbString(fg), background: rgbString(rootBg), confident: true };
}

function parseColor(value: string): { r: number; g: number; b: number; a: number } | null {
  const match = /rgba?\(([^)]+)\)/.exec(value);
  if (!match) return null;
  const parts = match[1].split(/[\s,/]+/).filter(Boolean).map(Number);
  if (parts.length < 3 || parts.slice(0, 3).some(n => !Number.isFinite(n))) return null;
  return { r: parts[0], g: parts[1], b: parts[2], a: parts[3] === undefined ? 1 : parts[3] };
}

function rgbString(c: { r: number; g: number; b: number }): string {
  return `rgb(${Math.round(c.r)}, ${Math.round(c.g)}, ${Math.round(c.b)})`;
}

function luminanceRatio(a: { r: number; g: number; b: number }, b: { r: number; g: number; b: number }): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  const light = Math.max(la, lb);
  const dark = Math.min(la, lb);
  return (light + 0.05) / (dark + 0.05);
}

function relativeLuminance({ r, g, b }: { r: number; g: number; b: number }): number {
  const channel = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}
