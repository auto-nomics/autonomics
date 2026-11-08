import { useEffect, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import type { DeckPayload } from "../deck-api";
import { clickTotal } from "../render/deck-model";
import { SlideView } from "../render/slide-view";
import { stepNav, type NavState, type PresentationMessage } from "../render/presentation-sync";
import "../deck.css";

function Preview({ payload, nav, dark }: { payload: DeckPayload; nav: NavState; dark: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const [scale, setScale] = useState(1);
  const width = payload.canvasSize?.width ?? payload.deck.canvasWidth ?? 980;
  const ratio = payload.deck.aspectRatio ?? { width: 16, height: 9 };
  const height = payload.canvasSize?.height ?? Math.round(width * ratio.height / ratio.width);
  const slide = payload.slides[nav.slide - 1];
  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    const update = () => setScale(Math.min(node.clientWidth / width, node.clientHeight / height));
    const observer = new ResizeObserver(update);
    observer.observe(node); update();
    return () => observer.disconnect();
  }, [width, height]);
  if (!slide) return <div className="presenter-preview" ref={ref}>没有幻灯片</div>;
  const slideDark = payload.deck.colorSchema === "dark" ? true : payload.deck.colorSchema === "light" ? false : dark;
  return <div className="presenter-preview" ref={ref}><div className="deck-canvas" style={{ width, height, transform: `scale(${scale})` }}>
    <SlideView slide={slide} deckId={payload.deck.id} assets={payload.assets} themeCss={payload.themeCss} outline={payload.outline} currentClick={Math.min(Math.max(nav.click, slide.clickPlan?.clicksStart ?? 0), clickTotal(slide))} dark={slideDark} />
  </div></div>;
}

function Presenter({ session }: { session: string }) {
  const [state, setState] = useState<{ payload: DeckPayload; nav: NavState; dark: boolean } | null>(null);
  const [connected, setConnected] = useState(false);
  const [running, setRunning] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const channel = useRef<BroadcastChannel | null>(null);
  useEffect(() => {
    if (!session || typeof BroadcastChannel === "undefined") return;
    const c = new BroadcastChannel(`deck-presenter-${session}`);
    channel.current = c;
    let lastState = 0;
    c.onmessage = (event: MessageEvent<PresentationMessage>) => {
      if (event.data.type === "state") { lastState = Date.now(); setState(event.data); setConnected(true); }
      else if (event.data.type === "closed") setConnected(false);
    };
    c.postMessage({ type: "ready" } satisfies PresentationMessage);
    // Re-establish state if this window was refreshed while its opener is alive.
    const retry = window.setInterval(() => {
      if (lastState && Date.now() - lastState > 8000) setConnected(false);
      c.postMessage({ type: "ready" } satisfies PresentationMessage);
    }, 3000);
    return () => { window.clearInterval(retry); c.close(); channel.current = null; };
  }, [session]);
  useEffect(() => {
    if (!running) return;
    const start = Date.now() - elapsed;
    const timer = window.setInterval(() => setElapsed(Date.now() - start), 250);
    return () => window.clearInterval(timer);
  }, [running]); // elapsed is captured when the timer starts
  const step = (delta: 1 | -1) => channel.current?.postMessage({ type: "step", delta } satisfies PresentationMessage);
  useEffect(() => {
    const handle = (e: KeyboardEvent) => {
      if (e.target instanceof Element && e.target.closest("input, textarea, select, [contenteditable=true]")) return;
      if (e.key === "ArrowRight" || e.key === "PageDown" || (e.key === " " && !(e.target instanceof Element && e.target.closest("button")))) { e.preventDefault(); step(1); }
      else if (e.key === "ArrowLeft" || e.key === "PageUp") { e.preventDefault(); step(-1); }
    };
    window.addEventListener("keydown", handle);
    return () => window.removeEventListener("keydown", handle);
  }, []);
  const seconds = Math.floor(elapsed / 1000);
  const time = [Math.floor(seconds / 3600), Math.floor(seconds / 60) % 60, seconds % 60].map(n => String(n).padStart(2, "0")).join(":");
  const next = state && stepNav(state.nav, state.payload.slides, 1);
  const atEnd = state && next && next.slide === state.nav.slide && next.click === state.nav.click;
  return <div className="presenter">
    <header className="presenter-header"><div><strong>讲演者模式</strong><span className="presenter-title">{state?.payload.deck.title}</span></div><span className={connected ? "presenter-connected" : "presenter-disconnected"}>{connected ? "已连接观众画面" : "等待观众画面连接"}</span></header>
    {state ? <div className="presenter-grid">
      <section className="presenter-current"><div className="presenter-label">当前画面 <span>{state.nav.slide} / {state.payload.slides.length}</span></div><Preview payload={state.payload} nav={state.nav} dark={state.dark} /></section>
      <section className="presenter-next"><div className="presenter-label">接下来 <span>{atEnd ? "已到末尾" : next?.slide === state.nav.slide ? "下一动画" : "下一页"}</span></div>{next && <Preview payload={state.payload} nav={next} dark={state.dark} />}</section>
      <section className="presenter-notes"><div className="presenter-label">演讲备注</div><div className="presenter-notes-body">{state.payload.slides[state.nav.slide - 1]?.note || "此页没有备注"}</div></section>
    </div> : <div className="presenter-wait">请从幻灯片的“讲演者模式”按钮打开此窗口。</div>}
    <footer className="presenter-footer"><div className="presenter-navigation"><button type="button" onClick={() => step(-1)} disabled={!connected}>上一动画 / 页</button><button type="button" onClick={() => step(1)} disabled={!connected}>下一动画 / 页</button></div><div className="presenter-timer"><span aria-label="计时">{time}</span><button type="button" onClick={() => setRunning(value => !value)}>{running ? "暂停" : "开始"}</button><button type="button" onClick={() => { setRunning(false); setElapsed(0); }}>重置</button></div></footer>
  </div>;
}

export function bootPresenterPage(container: HTMLDivElement): void {
  const params = new URLSearchParams(location.search);
  const session = params.get("session") ?? params.get("deckPresenter") ?? "";
  ReactDOM.createRoot(container).render(<Presenter session={session} />);
}
