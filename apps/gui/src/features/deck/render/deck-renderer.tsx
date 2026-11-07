/**
 * DeckRenderer — the deck shell: fixed-canvas scaling, keyboard
 * navigation (arrows/space step clicks then slides), click counter, and
 * the slide thumbnail rail.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, Presentation } from "lucide-react";
import type { DeckPayload, SlideSpec } from "../deck-api";
import { clickTotal } from "./deck-model";
import { SlideView } from "./slide-view";

export interface DeckRendererProps {
  payload: DeckPayload;
  dark: boolean;
  /** 1-based start slide (from the opener). */
  startSlide?: number;
}

interface NavState {
  slide: number; // 1-based
  click: number;
}

export function DeckRenderer({ payload, dark, startSlide }: DeckRendererProps): React.ReactNode {
  const { deck, slides } = payload;
  const canvasWidth = deck.canvasWidth ?? 980;
  const aspect = deck.aspectRatio ?? { width: 16, height: 9 };
  const canvasHeight = Math.round((canvasWidth * aspect.height) / aspect.width);

  const [nav, setNav] = useState<NavState>({
    slide: Math.min(Math.max(startSlide ?? 1, 1), Math.max(slides.length, 1)),
    click: 0,
  });
  const stageRef = useRef<HTMLDivElement | null>(null);
  const [scale, setScale] = useState(1);

  const slide: SlideSpec | undefined = slides[nav.slide - 1];

  // Canvas → stage scaling (ResizeObserver keeps it exact).
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const update = () => {
      const rect = stage.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) return;
      const s = Math.min(rect.width / canvasWidth, rect.height / canvasHeight);
      setScale(s > 0 ? s : 1);
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(stage);
    return () => ro.disconnect();
  }, [canvasWidth, canvasHeight]);

  const total = slide ? clickTotal(slide) : 0;
  const clampedClick = Math.min(Math.max(nav.click, slide?.clickPlan?.clicksStart ?? 0), total);

  const go = useCallback(
    (delta: 1 | -1) => {
      setNav(({ slide, click }) => {
        const s = slides[slide - 1];
        const t = s ? clickTotal(s) : 0;
        const c = Math.min(Math.max(click, s?.clickPlan?.clicksStart ?? 0), t);
        if (delta === 1) {
          if (c < t) return { slide, click: c + 1 };
          if (slide < slides.length) return { slide: slide + 1, click: 0 };
          return { slide, click: c };
        }
        if (c > (s?.clickPlan?.clicksStart ?? 0)) return { slide, click: c - 1 };
        if (slide > 1) {
          const prev = slides[slide - 2];
          const pt = prev ? clickTotal(prev) : 0;
          return { slide: slide - 1, click: pt };
        }
        return { slide, click: c };
      });
    },
    [slides],
  );

  // Keyboard: slidev-style arrows/space navigation while the stage has focus.
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      if (e.key === "ArrowRight" || e.key === " " || e.key === "PageDown") {
        e.preventDefault();
        go(1);
      } else if (e.key === "ArrowLeft" || e.key === "PageUp") {
        e.preventDefault();
        go(-1);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [go]);

  // Click visibility re-applied inside SlideView; reset click on slide change.
  useEffect(() => {
    setNav(n => ({ ...n, click: Math.min(n.click, clickTotal(slides[n.slide - 1] ?? ({} as SlideSpec)) || 0) }));
  }, [nav.slide, slides]);

  const notes = slide?.note ?? null;
  const toc = useMemo(
    () =>
      slides.map(s => ({
        no: s.index + 1,
        title: s.title ?? `(${s.index + 1})`,
        level: s.level ?? 1,
      })),
    [slides],
  );

  return (
    <div className={`deck-renderer ${dark ? "deck-renderer--dark" : "deck-renderer--light"}`}>
      <aside className="deck-rail">
        <div className="deck-rail-head">
          <Presentation size={14} />
          <span className="deck-rail-title">{deck.title || "未命名"}</span>
        </div>
        <div className="deck-toc">
          {toc.map(item => (
            <button
              key={item.no}
              type="button"
              className={`deck-toc-item ${item.no === nav.slide ? "deck-toc-item--active" : ""}`}
              onClick={() => setNav({ slide: item.no, click: 0 })}
            >
              <span className="deck-toc-no">{item.no}</span>
              <span className="deck-toc-label" style={{ paddingLeft: (item.level - 1) * 10 }}>
                {item.title}
              </span>
            </button>
          ))}
        </div>
      </aside>

      <main className="deck-main">
        <div className="deck-stage" ref={stageRef} data-deck-stage="">
          <div
            className="deck-canvas"
            style={{
              width: canvasWidth,
              height: canvasHeight,
              transform: `scale(${scale})`,
            }}
          >
            {slide ? (
              <SlideView
                key={`${slide.revision}:${slide.index}`}
                slide={slide}
                deckId={deck.id}
                assets={payload.assets}
                currentClick={clampedClick}
                dark={dark}
              />
            ) : (
              <div className="deck-empty">空 deck</div>
            )}
          </div>
        </div>

        <div className="deck-controls">
          <button type="button" className="deck-btn" onClick={() => go(-1)} aria-label="上一页">
            <ChevronLeft size={16} />
          </button>
          <span className="deck-counter">
            {nav.slide} / {slides.length}
            {total > 0 ? <span className="deck-click-counter"> · click {clampedClick}/{total}</span> : null}
          </span>
          <button type="button" className="deck-btn" onClick={() => go(1)} aria-label="下一页">
            <ChevronRight size={16} />
          </button>
        </div>

        {notes ? (
          <div className="deck-notes">
            <div className="deck-notes-head">备注</div>
            <div className="deck-notes-body">{notes}</div>
          </div>
        ) : null}
      </main>
    </div>
  );
}
