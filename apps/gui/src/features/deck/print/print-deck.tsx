/**
 * PrintDeck — renders slides full-bleed at exact canvas size for
 * capture. For PDF export the print stylesheet gives each slide its own
 * `@page`-sized block; for PNG the exporter drives `__deckGoto` between
 * shots. Markdown/KaTeX render synchronously; mermaid keeps a
 * `[data-waitfor]` sentinel up until its SVGs settle.
 */
import { useEffect, useMemo, useState } from "react";
import type { PrintPayload } from "../deck-api-print";
import { printAssetUrl } from "../deck-api-print";
import { SlideView } from "../render/slide-view";
import { renderMermaidBlocks } from "../render/mermaid";

export interface PrintDeckProps {
  payload: PrintPayload;
  token: string;
  revealAllClicks: boolean;
  /** "single" = one stage driven by __deckGoto (PNG/PPTX capture);
   *  "pdf" = every slide stacked at page height — printToPDF paginates
   *  one slide per page (slidev's one-piece mode). */
  mode: "single" | "pdf";
}

declare global {
  interface Window {
    __deckGoto?: (slide: number, click: number) => void;
    __deckState?: { slide: number; click: number };
  }
}

export function PrintDeck({ payload, token, revealAllClicks, mode }: PrintDeckProps): React.ReactNode {
  const { deck, slides, assets } = payload;
  const canvasWidth = deck.canvasWidth ?? 980;
  const aspect = deck.aspectRatio ?? { width: 16, height: 9 };
  const canvasHeight = Math.round((canvasWidth * aspect.height) / aspect.width);

  const [state, setState] = useState({ slide: 1, click: 0 });
  const rootRef = useMemo(() => ({ current: document.getElementById("app") }), []);

  // Expose the exporter's navigation hook.
  useEffect(() => {
    window.__deckState = state;
    window.__deckGoto = (slide: number, click: number) => {
      setState({ slide, click });
    };
    return () => {
      window.__deckGoto = undefined;
    };
  }, [state]);

  const assetsForPrint = useMemo(
    () =>
      assets.map(a => ({
        ...a,
        // SlideView resolves assets://{id} through the plain raw URL; for
        // print pages the token-authenticated sibling is required.
        ref: a.ref,
      })),
    [assets],
  );

  // Global asset resolution override: SlideView's markdown pipeline maps
  // assets:// to /assets/{id}/raw (session route). Monkey-patching is
  // fragile — instead the print deck renders with the same pipeline but a
  // custom resolver is not plumbed; the exporter (loopback daemon) HAS a
  // session-free path only via print-assets. Handle it here: rewrite any
  // session raw URL to the token URL once the DOM settles.
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    root.querySelectorAll<HTMLImageElement>("img[src*='/assets/']").forEach(img => {
      const match = /\/deck\/decks\/([^/]+)\/assets\/([^/]+)\/raw/.exec(img.src);
      if (match) {
        img.src = printAssetUrl(match[1], match[2], token);
      }
    });
  });

  // Wait protocol: mermaid renders, in-flight images complete, the
  // [data-waitfor] sentinel drains, then deckReady flips. Re-runs per
  // slide so per-page captures re-arm the readiness gate.
  useEffect(() => {
    let cancelled = false;
    document.documentElement.dataset.deckReady = "false";
    const sentinel = document.createElement("div");
    sentinel.setAttribute("data-waitfor", "");
    sentinel.style.display = "none";
    document.body.appendChild(sentinel);

    const waitForImages = () => {
      const imgs = Array.from(document.images);
      return Promise.all(
        imgs.map(img =>
          img.complete ? Promise.resolve() : new Promise<void>(resolve => {
            img.addEventListener("load", () => resolve(), { once: true });
            img.addEventListener("error", () => resolve(), { once: true });
          }),
        ),
      );
    };

    const finish = () => {
      if (cancelled) return;
      waitForImages().then(() => {
        if (cancelled) return;
        sentinel.remove();
        document.documentElement.dataset.deckReady = "true";
      });
    };

    const node = document.getElementById("app");
    if (node) {
      renderMermaidBlocks(node, deck.colorSchema === "dark").then(finish).catch(finish);
    } else {
      finish();
    }
    return () => {
      cancelled = true;
      sentinel.remove();
    };
  }, [state.slide, mode, deck.colorSchema]);

  const slide = slides[state.slide - 1];
  const dark = deck.colorSchema === "dark";

  if (mode === "pdf") {
    return (
      <>
        <style>{`
          html, body, #app { margin: 0; padding: 0; background: #fff; }
          .deck-print-page {
            width: ${canvasWidth}px;
            height: ${canvasHeight}px;
            overflow: hidden;
            page-break-after: always;
            break-after: page;
            position: relative;
          }
          @page { size: ${canvasWidth}px ${canvasHeight}px; margin: 0; }
        `}</style>
        <div data-deck-canvas-width={canvasWidth} data-deck-canvas-height={canvasHeight}>
          {slides.map(s => (
            <div className="deck-print-page" key={s.revision + s.index}>
              <SlideView
                slide={s}
                deckId={deck.id}
                assets={assetsForPrint}
                currentClick={revealAllClicks ? Number.MAX_SAFE_INTEGER : (s.clickPlan?.total ?? 0)}
                dark={dark}
                revealAll={revealAllClicks}
              />
            </div>
          ))}
        </div>
      </>
    );
  }

  return (
    <>
      <style>{`
        html, body, #app { margin: 0; padding: 0; background: #fff; }
        .deck-print-stage {
          width: ${canvasWidth}px;
          height: ${canvasHeight}px;
          overflow: hidden;
          position: fixed;
          inset: 0;
          margin: 0 auto;
        }
      `}</style>
      <div className="deck-print-stage" data-deck-canvas-width={canvasWidth} data-deck-canvas-height={canvasHeight}>
        {slide ? (
          <SlideView
            key={`${slide.revision}:${slide.index}:${state.click}`}
            slide={slide}
            deckId={deck.id}
            assets={assetsForPrint}
            currentClick={state.click}
            dark={dark}
            revealAll={revealAllClicks}
          />
        ) : null}
      </div>
    </>
  );
}
