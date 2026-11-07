/**
 * Slide-content markdown pipeline.
 *
 * Same math/GFM base as the app pipeline (`lib/markdown.ts`), plus:
 * - `assets://{id}` refs resolved to raw asset URLs before parsing;
 * - `<v-click>` markers annotated to `data-deck-click` spans (order must
 *   match the server's ClickPlan — see deck-model.ts);
 * - ```mermaid fences rendered by the lazy-loaded mermaid module.
 */
import DOMPurify from "dompurify";
import { createMarkdownPipeline, escapeHtml } from "@/lib/markdown";
import type { AssetMeta } from "../deck-api";
import { annotateClickElements } from "./deck-model";
import { renderMermaidBlocks } from "./mermaid";

const deckPipeline = createMarkdownPipeline();

export interface SlideRenderOptions {
  deckId: string;
  assets: AssetMeta[];
}

/** Parse one slide's markdown into sanitized HTML with annotations. */
export function renderSlideContent(content: string, options: SlideRenderOptions): string {
  const resolved = resolveAssetRefs(content, options.assets, options.deckId);
  const parsed = deckPipeline.parse(resolved);
  const annotated = annotateClickElements(parsed);
  // Mermaid fences → placeholder divs (lazy client render); sync blocks
  // render inline via the shared pipeline's code renderer otherwise.
  const sanitized = DOMPurify.sanitize(annotated, {
    USE_PROFILES: { html: true, mathMl: true },
    ADD_TAGS: ["semantics", "annotation"],
    // data-deck-click / data-deck-at / data-mermaid survive sanitize.
    ADD_ATTR: ["data-deck-click", "data-deck-at", "data-mermaid", "target"],
  });
  return sanitized;
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
