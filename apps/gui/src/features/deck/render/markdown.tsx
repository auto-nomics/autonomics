/**
 * Slide-content markdown pipeline — Comark edition.
 *
 * Same shared base as the agent/wiki pipelines (`lib/comark-markdown`):
 * math, breaks, GFM, the `.md-code` shell — plus deck specifics:
 * - `assets://{id}` refs resolved to raw asset URLs before parsing;
 * - `<v-click>` markers annotated to `data-deck-click` spans after
 *   rendering (order must match the server's ClickPlan — see
 *   deck-model.ts). Comark passes raw HTML (tags and attributes)
 *   straight through, so the annotation regexes see the same shapes the
 *   marked pipeline produced;
 * - ```mermaid fences live-render client-side by the lazy mermaid module
 *   (the Rust engine wraps them as `deck-mermaid-holder` blocks).
 *
 * Rendering is async (Comark decouples parse from render); SlideView
 * consumes it through `useSlideContent`, which keeps the last frame
 * while the next one renders.
 */
import { useEffect, useState } from "react";
import { createHtmlRenderer } from "@comark/html";
import DOMPurify from "dompurify";
import { comarkBasePlugins, comarkBaseComponents } from "@/lib/comark-markdown";
import { escapeHtml } from "@/lib/markdown";
import type { AssetMeta } from "../deck-api";
import { annotateClickElements } from "./deck-model";
import { renderMermaidBlocks } from "./mermaid";

const deckRenderer = createHtmlRenderer({
  plugins: comarkBasePlugins,
  components: comarkBaseComponents,
});

export interface SlideRenderOptions {
  deckId: string;
  assets: AssetMeta[];
}

/** Parse one slide's markdown into sanitized HTML with annotations. */
export async function renderSlideContent(content: string, options: SlideRenderOptions): Promise<string> {
  const resolved = resolveAssetRefs(content, options.assets, options.deckId);
  const parsed = await deckRenderer(resolved);
  const annotated = annotateClickElements(parsed);
  // Mermaid fences → placeholder divs (lazy client render); sync blocks
  // render inline via the shared pipeline's code renderer otherwise.
  return DOMPurify.sanitize(annotated, {
    USE_PROFILES: { html: true, mathMl: true },
    ADD_TAGS: ["semantics", "annotation"],
    // data-deck-click / data-deck-at / data-mermaid survive sanitize.
    ADD_ATTR: ["data-deck-click", "data-deck-at", "data-mermaid", "target"],
  });
}

/**
 * Per-slide rendered HTML. Keeps the last frame while the next one
 * renders; DOM passes in SlideView (mermaid, click visibility) key on
 * the returned HTML, so they re-run after markup lands.
 */
export function useSlideContent(content: string, options: SlideRenderOptions): string {
  const { deckId, assets } = options;
  const [html, setHtml] = useState("");
  useEffect(() => {
    let cancelled = false;
    renderSlideContent(content, { deckId, assets }).then((next) => {
      if (!cancelled) setHtml(next);
    });
    return () => {
      cancelled = true;
    };
  }, [content, deckId, assets]);
  return html;
}

function resolveAssetRefs(content: string, assets: AssetMeta[], deckId: string): string {
  const byId = new Map(assets.map(a => [a.assetId, a]));
  return content.replace(/assets:\/\/([A-Za-z0-9_-]+)/g, (full, id: string) => {
    const asset = byId.get(id);
    if (!asset) return full;
    return `/api/v1/deck/decks/${encodeURIComponent(deckId)}/assets/${encodeURIComponent(id)}/raw`;
  });
}

/** Extract the image URL for figure layouts from content or frontmatter. */
export function figureImageSrc(
  fmImage: string | null | undefined,
  options: SlideRenderOptions,
): string | null {
  if (!fmImage) return null;
  const resolved = resolveAssetRefs(fmImage, options.assets, options.deckId);
  return resolved;
}

export { escapeHtml, renderMermaidBlocks };
