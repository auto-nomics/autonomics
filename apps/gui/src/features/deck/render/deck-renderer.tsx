/**
 * DeckRenderer — the deck shell: fixed-canvas scaling, keyboard
 * navigation (arrows/space step clicks then slides), click counter, and
 * the slide thumbnail rail.
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, Maximize, Minimize, Presentation } from "lucide-react";
import type { DeckPayload, SlideSpec } from "../deck-api";
import { clickTotal } from "./deck-model";
import { SlideView } from "./slide-view";
import { isTauri } from "@/lib/is-tauri";
import { stepNav, type NavState, type PresentationMessage } from "./presentation-sync";

export interface DeckRendererProps {
  payload: DeckPayload;
  dark: boolean;
  /** 1-based start slide (from the opener). */
  startSlide?: number;
  onSlideChange?: (slide: number) => void;
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
  const rendererRef = useRef<HTMLDivElement | null>(null);
  const channelRef = useRef<BroadcastChannel | null>(null);
  const popupRef = useRef<Window | null>(null);
  const nativePresenterRef = useRef<import("@tauri-apps/api/webviewWindow").WebviewWindow | null>(null);
  const [fullscreen, setFullscreen] = useState(false);
  const [presenterError, setPresenterError] = useState("");
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
      setNav(current => stepNav(current, slides, delta));
    },
    [slides],
  );

  const toggleFullscreen = useCallback(() => {
    if (document.fullscreenElement === rendererRef.current) {
      void document.exitFullscreen().catch(() => setPresenterError("无法退出全屏"));
    } else if (rendererRef.current?.requestFullscreen) {
      setZoom(1); setPan({ x: 0, y: 0 });
      void rendererRef.current.requestFullscreen().catch(() => setPresenterError("无法进入全屏，请检查浏览器权限"));
    } else setPresenterError("当前环境不支持全屏展示");
  }, []);

  useEffect(() => {
    const update = () => setFullscreen(document.fullscreenElement === rendererRef.current);
    document.addEventListener("fullscreenchange", update);
    return () => document.removeEventListener("fullscreenchange", update);
  }, []);

  const stateRef = useRef({ payload, nav, dark });
  stateRef.current = { payload, nav, dark };
  useEffect(() => {
    channelRef.current?.postMessage({ type: "state", payload, nav, dark } satisfies PresentationMessage);
  }, [payload, nav, dark]);
  useEffect(() => () => {
    channelRef.current?.postMessage({ type: "closed" } satisfies PresentationMessage);
    channelRef.current?.close();
  }, []);

  const openPresenter = () => {
    if (popupRef.current && !popupRef.current.closed) { popupRef.current.focus(); return; }
    if (nativePresenterRef.current) { void nativePresenterRef.current.setFocus().catch(() => { nativePresenterRef.current = null; }); return; }
    if (typeof BroadcastChannel === "undefined") { setPresenterError("当前环境不支持讲演者窗口同步"); return; }
    const session = crypto.randomUUID();
    const channel = new BroadcastChannel(`deck-presenter-${session}`);
    channelRef.current?.close();
    channelRef.current = channel;
    channel.onmessage = (event: MessageEvent<PresentationMessage>) => {
      const message = event.data;
      if (message.type === "ready") channel.postMessage({ type: "state", ...stateRef.current } satisfies PresentationMessage);
      else if (message.type === "step") setNav(current => stepNav(current, stateRef.current.payload.slides, message.delta));
      else if (message.type === "goto" && Number.isInteger(message.slide) && message.slide >= 1 && message.slide <= stateRef.current.payload.slides.length) setNav({ slide: message.slide, click: 0 });
    };
    if (isTauri) {
      void import("@tauri-apps/api/webviewWindow").then(async ({ WebviewWindow }) => {
        const presenter = new WebviewWindow(`deck-presenter-${session}`, {
          url: `index.html?deckPresenter=${encodeURIComponent(session)}`,
          title: "讲演者模式", width: 1200, height: 800,
        });
        nativePresenterRef.current = presenter;
        await presenter.once("tauri://error", event => {
          nativePresenterRef.current = null;
          channel.close();
          if (channelRef.current === channel) channelRef.current = null;
          setPresenterError(`无法打开讲演者窗口：${String(event.payload)}`);
        });
        await presenter.onCloseRequested(() => { nativePresenterRef.current = null; });
      }).catch(error => { channel.close(); channelRef.current = null; setPresenterError(`无法打开讲演者窗口：${String(error)}`); });
      setPresenterError("");
      return;
    }
    const url = new URL("/deck/presenter", location.href);
    url.searchParams.set("session", session);
    const popup = window.open(url.href, "_blank", "popup,width=1200,height=800");
    if (!popup) { channel.close(); channelRef.current = null; setPresenterError("讲演者窗口被拦截，请允许弹出窗口后重试"); return; }
    popupRef.current = popup;
    setPresenterError("");
  };

  // Keyboard: slidev-style arrows/space navigation while the stage has focus.
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      // Space activates focused controls (for example an outline expander)
      // and must not simultaneously advance the presentation.
      if (e.key === " " && e.target instanceof Element && e.target.closest("button, a, select, [contenteditable='true']")) return;
      if (e.key.toLowerCase() === "f" && !e.ctrlKey && !e.metaKey && !e.altKey && !(e.target instanceof Element && e.target.closest("[contenteditable=true]"))) { e.preventDefault(); toggleFullscreen(); }
      else if (e.key === "ArrowRight" || e.key === " " || e.key === "PageDown") {
        e.preventDefault();
        go(1);
      } else if (e.key === "ArrowLeft" || e.key === "PageUp") {
        e.preventDefault();
        go(-1);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [go, toggleFullscreen]);

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
    <div ref={rendererRef} className={`deck-renderer ${dark ? "deck-renderer--dark" : "deck-renderer--light"}`}>
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

        {presenterError && <div className="deck-presentation-error" role="alert">{presenterError} <button type="button" onClick={() => setPresenterError("")}>×</button></div>}
        <div className="deck-controls">
          <button type="button" className="deck-btn" onClick={openPresenter} aria-label="打开讲演者模式" title="打开讲演者模式"><Presentation size={16} /></button>
          <button type="button" className="deck-btn" onClick={toggleFullscreen} aria-label={fullscreen ? "退出全屏" : "全屏展示"} title="全屏展示 (F)">{fullscreen ? <Minimize size={16} /> : <Maximize size={16} />}</button>
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
