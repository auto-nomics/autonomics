/**
 * DeckRenderer — the deck shell: fixed-canvas scaling, keyboard
 * navigation (arrows/space step clicks then slides), click counter, and
 * the slide thumbnail rail.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import type { DeckPayload, SlideSpec } from "../deck-api";
import { clickTotal } from "./deck-model";
import { SlideView } from "./slide-view";

export interface DeckRendererProps {
  payload: DeckPayload;
  dark: boolean;
  /** 1-based start slide (from the opener). */
  startSlide?: number;
  onSlideChange?: (slide: number) => void;
}

interface NavState {
  slide: number; // 1-based
  click: number;
}

export function DeckRenderer({ payload, dark, startSlide, onSlideChange }: DeckRendererProps): React.ReactNode {
  const { deck, slides } = payload;
  const canvasWidth = payload.canvasSize?.width ?? deck.canvasWidth ?? 980;
  const aspect = deck.aspectRatio ?? { width: 16, height: 9 };
  const canvasHeight = payload.canvasSize?.height ?? Math.round((canvasWidth * aspect.height) / aspect.width);
  const slideDark = deck.colorSchema === "dark" ? true : deck.colorSchema === "light" ? false : dark;

  const [nav, setNav] = useState<NavState>({
    slide: Math.min(Math.max(startSlide ?? 1, 1), Math.max(slides.length, 1)),
    click: 0,
  });
  const stageRef = useRef<HTMLDivElement | null>(null);
  const [scale, setScale] = useState(1);
  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState({ x: 0, y: 0 });

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

  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const unit = event.deltaMode === WheelEvent.DOM_DELTA_LINE ? 16 : event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? stage.clientHeight : 1;
      const dx = event.deltaX * unit;
      const dy = event.deltaY * unit;
      if (event.ctrlKey || event.metaKey) {
        setZoom(value => Math.min(4, Math.max(0.25, value * Math.exp(-dy * 0.002))));
      } else {
        setPan(value => ({ x: value.x - (event.shiftKey ? (dy || dx) : dx), y: value.y - (event.shiftKey ? 0 : dy) }));
      }
    };
    stage.addEventListener("wheel", onWheel, { passive: false });
    return () => stage.removeEventListener("wheel", onWheel);
  }, []);

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
      // Space activates focused controls (for example an outline expander)
      // and must not simultaneously advance the presentation.
      if (e.key === " " && e.target instanceof Element && e.target.closest("button, a, select, [contenteditable='true']")) return;
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
  useEffect(() => {
    window.dispatchEvent(new CustomEvent("deck-slide-changed", { detail: { deckId: deck.id, slide: nav.slide } }));
    onSlideChange?.(nav.slide);
  }, [deck.id, nav.slide, onSlideChange]);
  useEffect(() => {
    const handler = (event: Event) => {
      const detail = (event as CustomEvent<{ deckId: string; slide: number }>).detail;
      if (detail.deckId === deck.id) setNav({ slide: detail.slide, click: 0 });
    };
    window.addEventListener("deck-go-slide", handler);
    return () => window.removeEventListener("deck-go-slide", handler);
  }, [deck.id]);

  return (
    <div className={`deck-renderer ${dark ? "deck-renderer--dark" : "deck-renderer--light"}`}>
      <main className="deck-main">
        <div className="deck-stage" ref={stageRef} data-deck-stage="">
          <div
            className="deck-canvas"
            style={{
              width: canvasWidth,
              height: canvasHeight,
              transform: `translate(${pan.x}px, ${pan.y}px) scale(${scale * zoom})`,
            }}
          >
            {slide ? (
              <SlideView
                key={`${slide.revision}:${slide.index}`}
                slide={slide}
                themeCss={payload.themeCss}
                outline={payload.outline}
                onNavigate={page => setNav({ slide: page, click: 0 })}
                deckId={deck.id}
                assets={payload.assets}
                currentClick={clampedClick}
                dark={slideDark}
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
          <button type="button" className="deck-btn" onClick={() => { setZoom(1); setPan({ x: 0, y: 0 }); }} title="重置画布缩放与位置" aria-label="重置画布缩放">{Math.round(zoom * 100)}%</button>
          <button type="button" className="deck-btn" onClick={() => go(1)} aria-label="下一页">
            <ChevronRight size={16} />
          </button>
        </div>

        {slides.some(s => s.note) ? (
          <div className="deck-notes">
            <div className="deck-notes-head">备注</div>
            <div className="deck-notes-body">{notes || "此页没有备注"}</div>
          </div>
        ) : null}
      </main>
    </div>
  );
}
