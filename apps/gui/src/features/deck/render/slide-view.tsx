/**
 * SlideView — one slide on the fixed canvas (Slidev's canvasWidth
 * scaled to the viewport). Renders markdown → HTML once per revision,
 * applies click visibility (Slidev CSS-class semantics), and
 * lazy-renders mermaid blocks. Layout arrangement is CSS-driven via the
 * `data-layout` attribute (see deck.css) — figure/panel slots render as
 * sibling elements the grid positions.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import type { AssetMeta, SlideSpec } from "../deck-api";
import { figureImageSrc, renderSlideContent, renderMermaidBlocks } from "./markdown";
import { applyClickVisibility, CLICK_HIDDEN } from "./deck-model";

export interface SlideViewProps {
  slide: SlideSpec;
  deckId: string;
  assets: AssetMeta[];
  /** Current click position (clamped by the caller). */
  currentClick: number;
  dark: boolean;
  /** Reveal every click element regardless of position (export mode). */
  revealAll?: boolean;
}

export function SlideView({
  slide,
  deckId,
  assets,
  currentClick,
  dark,
  revealAll,
}: SlideViewProps): React.ReactNode {
  const html = useMemo(
    () => renderSlideContent(slide.content, { deckId, assets }),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- revision pins content
    [slide.revision, deckId, assets],
  );
  const figureSrc = useMemo(
    () => figureImageSrc(slide.frontmatter.image, { deckId, assets }),
    [slide.frontmatter.image, deckId, assets],
  );
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const [mermaidDone, setMermaidDone] = useState(false);

  const layout = String(slide.frontmatter.layout ?? (slide.index === 0 ? "cover" : "default"));

  // Mermaid renders after the HTML mounts (no-op without fences).
  useEffect(() => {
    setMermaidDone(false);
    const node = bodyRef.current;
    if (!node) return;
    let cancelled = false;
    renderMermaidBlocks(node, dark).then(() => {
      if (!cancelled) setMermaidDone(true);
    });
    return () => {
      cancelled = true;
    };
  }, [html, dark]);

  // Click visibility runs on the live DOM (CSS-class toggling).
  useEffect(() => {
    const node = bodyRef.current;
    if (!node) return;
    if (revealAll) {
      node.querySelectorAll<HTMLElement>("[data-deck-click]").forEach(el => {
        el.classList.remove(CLICK_HIDDEN);
      });
      return;
    }
    applyClickVisibility(node, slide.clickPlan, currentClick);
  }, [slide.clickPlan, currentClick, html, mermaidDone, revealAll]);

  return (
    <div
      className="deck-slide"
      data-layout={layout}
      data-slide-no={slide.index + 1}
    >
      {figureSrc && (layout === "figure-left" || layout === "figure-right" || layout === "image-left" || layout === "image-right") ? (
        <figure className="deck-figure">
          <img src={figureSrc} alt={slide.title ?? "figure"} className="deck-figure-img" />
          {slide.frontmatter.tableCaption ? (
            <figcaption className="deck-caption">{slide.frontmatter.tableCaption}</figcaption>
          ) : null}
        </figure>
      ) : null}
      {layout === "panel-grid" ? (
        <div className="deck-panel-grid" aria-hidden>
          {["A", "B", "C", "D"].map((letter, i) => (
            <div key={letter} className="deck-panel">
              <div className="deck-panel-letter">{letter}</div>
              <div className="deck-panel-caption">
                {(slide.frontmatter as Record<string, unknown>)[`panel${i + 1}` as const] as string | undefined ?? ""}
              </div>
            </div>
          ))}
        </div>
      ) : null}
      <div
        ref={bodyRef}
        className="deck-slide-body deck-prose"
        dangerouslySetInnerHTML={{ __html: html }}
      />
    </div>
  );
}
