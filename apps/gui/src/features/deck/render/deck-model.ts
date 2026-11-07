/**
 * Client-side click-directive scan — mirrors the Rust extractor
 * (`ppt-base/src/parser/directives.rs`) so DOM element order matches the
 * server-computed `ClickPlan` element order exactly.
 *
 * Supported authoring (raw HTML passthrough in markdown):
 *   <v-click>…</v-click>  <v-click at="3">…  <div v-click>  <span v-click="+2">
 */
import type { ClickPlan, SlideSpec } from "../deck-api";

const RE_COMPONENT = /<v-click\b([^>]*?)\/?>/g;
const RE_ATTR = /\sv-click(?:\s*=\s*("[^"]*"|'[^']*'|[^\s>]+))?(?:[\s/>]|$)/g;

/** Number of click-marked elements in a slide's content, in doc order. */
export function scanClickCount(content: string): number {
  const hits: number[] = [];
  for (const m of content.matchAll(RE_COMPONENT)) hits.push(m.index ?? 0);
  RE_ATTR.lastIndex = 0;
  for (const m of content.matchAll(RE_ATTR)) hits.push(m.index ?? 0);
  hits.sort((a, b) => a - b);
  return hits.length;
}

/**
 * Annotate the rendered HTML with `data-deck-click="{order}"` on every
 * click-marked element, converting `<v-click>` wrappers into `<span
 * data-deck-click>` pairs (DOMPurify keeps spans + data-*).
 */
export function annotateClickElements(html: string): string {
  // 1. Component form: <v-click ...> → <span data-deck-click="N">;
  //    </v-click> → </span>.
  let order = 0;
  let out = html.replace(/<v-click\b([^>]*?)\/?>/g, (_full, attrs: string) => {
    const at = parseAtAttr(attrs);
    const extra = at === undefined ? "" : ` data-deck-at="${escapeAttr(at)}"`;
    return `<span data-deck-click="${order++}"${extra}>`;
  });
  out = out.replace(/<\/v-click\s*>/g, "</span>");

  // 2. Attribute form: swap ` v-click(=value)?` for the annotation, keep
  //    the original trailing delimiter char the regex consumed.
  const chunks: string[] = [];
  let cursor = 0;
  RE_ATTR.lastIndex = 0;
  for (const m of out.matchAll(RE_ATTR)) {
    const start = m.index ?? 0;
    const end = start + m[0].length;
    chunks.push(out.slice(cursor, start));
    const value = m[1] ? stripQuotes(m[1]) : "";
    const extra = value === "" ? "" : ` data-deck-at="${escapeAttr(value)}"`;
    // The regex's trailing [\s/>] (or $) is the final char of m[0] —
    // preserve it so the tag stays well-formed.
    const tail = /[>\s/]$/.test(m[0]) ? m[0].slice(-1) : "";
    chunks.push(` data-deck-click="${order++}"${extra}${tail}`);
    cursor = end;
  }
  chunks.push(out.slice(cursor));
  return chunks.join("");
}

function stripQuotes(v: string): string {
  const t = v.trim();
  if (
    (t.startsWith('"') && t.endsWith('"') && t.length >= 2) ||
    (t.startsWith("'") && t.endsWith("'") && t.length >= 2)
  ) {
    return t.slice(1, -1);
  }
  return t;
}

function parseAtAttr(attrs: string): string | undefined {
  const m = /\bat\s*=\s*("([^"]*)"|'([^']*)'|([^\s>]+))/.exec(attrs);
  if (!m) return undefined;
  return m[2] ?? m[3] ?? m[4] ?? "";
}

function escapeAttr(v: string): string {
  return v.replaceAll("&", "&amp;").replaceAll('"', "&quot;").replaceAll("<", "&lt;");
}

// ── visibility ----------------------------------------------------------

export const CLICK_HIDDEN = "deck-click-hidden";
export const CLICK_CURRENT = "deck-click-current";
export const CLICK_PRIOR = "deck-click-prior";

/**
 * Slidev semantics: single (since) elements are active when current >=
 * start; range elements when start <= current < end.
 */
export function applyClickVisibility(
  root: HTMLElement,
  plan: ClickPlan | null,
  current: number,
): void {
  const nodes = root.querySelectorAll<HTMLElement>("[data-deck-click]");
  const order = plan ? plan.elements : [];
  nodes.forEach(node => {
    const idx = Number(node.dataset.deckClick ?? "-1");
    const state = order.find(e => e.order === idx);
    node.classList.remove(CLICK_HIDDEN, CLICK_CURRENT, CLICK_PRIOR);
    if (!state) return;
    const active =
      state.end === null
        ? current >= state.start
        : current >= state.start && current < state.end;
    if (!active) {
      node.classList.add(CLICK_HIDDEN);
    } else {
      if (current === state.start) node.classList.add(CLICK_CURRENT);
      else if (current > state.start) node.classList.add(CLICK_PRIOR);
    }
  });
}

/** Total clicks for a slide (plan or 0). */
export function clickTotal(slide: SlideSpec): number {
  return slide.clickPlan?.total ?? 0;
}
