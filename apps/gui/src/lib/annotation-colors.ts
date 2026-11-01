/**
 * Zotero-style annotation highlight palette. The single source of truth is
 * the `--annotation-*` custom properties in app.css (`:root`) — this helper
 * resolves them at runtime so TS and the swatch CSS never drift. Values are
 * theme-independent by design: they are stored with the annotation, so they
 * live outside the light/dark palettes.
 */

export const ANNOTATION_COLOR_NAMES = [
  "yellow",
  "red",
  "green",
  "blue",
  "purple",
  "magenta",
  "orange",
  "gray",
] as const;

export type AnnotationColorName = (typeof ANNOTATION_COLOR_NAMES)[number];

export const DEFAULT_ANNOTATION_COLOR: AnnotationColorName = "yellow";

/** Only used if the CSS custom properties cannot be read (no DOM). */
const FALLBACKS: Record<AnnotationColorName, string> = {
  yellow: "#ffd400",
  red: "#ff6666",
  green: "#5fb236",
  blue: "#2ea8e5",
  purple: "#a28ae5",
  magenta: "#e56eee",
  orange: "#f19837",
  gray: "#aaaaaa",
};

let cache: string[] | null = null;

/** All palette colors in display order (for swatch pickers). */
export function annotationPalette(): string[] {
  if (cache) return cache;
  cache = ANNOTATION_COLOR_NAMES.map((name) => annotationColor(name));
  return cache;
}

/** Resolve one palette color (hex) from the theme custom properties. */
export function annotationColor(name: string): string {
  const fallback = FALLBACKS[name as AnnotationColorName] ?? FALLBACKS.yellow;
  try {
    const value = getComputedStyle(document.documentElement)
      .getPropertyValue(`--annotation-${name}`)
      .trim();
    return value || fallback;
  } catch {
    return fallback;
  }
}

/** The default highlight color (Zotero yellow). */
export function defaultAnnotationColor(): string {
  return annotationColor(DEFAULT_ANNOTATION_COLOR);
}
