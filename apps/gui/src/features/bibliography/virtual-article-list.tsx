import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import type { BibArticle } from "@/types";

type Props = { articles: BibArticle[]; total: number; viewport: RefObject<HTMLDivElement | null>; contextKey: string; estimatedHeight: number; selectedId: string | null; onNavigate: (id: string) => void; onToggle: (id: string) => void; children: (article: BibArticle) => ReactNode };
function MeasuredRow({ id, top, index, total, onMeasure, children }: { id: string; top: number; index: number; total: number; onMeasure: (id: string, height: number) => void; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const node = ref.current!;
    const measure = () => onMeasure(id, Math.ceil(node.getBoundingClientRect().height));
    const observer = new ResizeObserver(measure);
    observer.observe(node); measure();
    return () => observer.disconnect();
  }, [id, onMeasure]);
  return <div ref={ref} role="listitem" aria-posinset={index + 1} aria-setsize={total} data-article-id={id} style={{ position: "absolute", top, left: 0, right: 0 }}>{children}</div>;
}
function containing(offsets: number[], position: number): number {
  let low = 0, high = Math.max(0, offsets.length - 2);
  while (low < high) { const middle = Math.ceil((low + high) / 2); if (offsets[middle] <= position) low = middle; else high = middle - 1; }
  return low;
}

/** Variable-height windowing keeps expanded attachments and snippets in the
 * normal keyboard/selection model while limiting mounted DOM rows. */
export function VirtualArticleList({ articles, total, viewport, contextKey, estimatedHeight, selectedId, onNavigate, onToggle, children }: Props) {
  const heights = useRef(new Map<string, number>());
  const activeContext = useRef(contextKey);
  const [version, setVersion] = useState(0);
  const [windowState, setWindow] = useState({ top: 0, height: 600 });
  const pendingFocus = useRef<string | null>(null);
  const [focusedId, setFocusedId] = useState<string | null>(null);
  const container = useRef<HTMLDivElement>(null);
  const layout = useMemo(() => {
    const offsets = [0];
    const positions = new Map<string, number>();
    articles.forEach((article, index) => { positions.set(article.id, index); offsets.push(offsets[index] + (heights.current.get(`${contextKey}\0${article.id}`) ?? estimatedHeight)); });
    return { offsets, positions };
  }, [articles, contextKey, estimatedHeight, version]);
  const layoutRef = useRef(layout); layoutRef.current = layout;
  useLayoutEffect(() => {
    if (activeContext.current !== contextKey) {
      activeContext.current = contextKey; heights.current.clear();
      if (viewport.current) viewport.current.scrollTop = 0;
      setWindow(current => ({ ...current, top: 0 })); setVersion(value => value + 1);
    }
  }, [contextKey, viewport]);
  useEffect(() => {
    const node = viewport.current;
    if (!node) return;
    const update = () => setWindow({ top: node.scrollTop, height: node.clientHeight || 600 });
    const observer = new ResizeObserver(update);
    observer.observe(node); node.addEventListener("scroll", update, { passive: true }); update();
    return () => { observer.disconnect(); node.removeEventListener("scroll", update); };
  }, [viewport]);
  const measure = useCallback((id: string, height: number) => {
    if (height <= 0 || activeContext.current !== contextKey) return;
    const key = `${contextKey}\0${id}`;
    const previous = heights.current.get(key) ?? estimatedHeight;
    if (heights.current.get(key) === height) return;
    heights.current.set(key, height);
    const index = layoutRef.current.positions.get(id);
    const node = viewport.current;
    if (node && index !== undefined && layoutRef.current.offsets[index] < node.scrollTop) node.scrollTop += height - previous;
    setVersion(value => value + 1);
  }, [contextKey, estimatedHeight, viewport]);
  const start = Math.max(0, containing(layout.offsets, windowState.top) - 6);
  const end = Math.min(articles.length, containing(layout.offsets, windowState.top + windowState.height) + 8);
  useLayoutEffect(() => {
    if (!pendingFocus.current) return;
    const row = [...(container.current?.querySelectorAll<HTMLElement>("[data-article-id]") ?? [])].find(row => row.dataset.articleId === pendingFocus.current);
    const button = row?.querySelector<HTMLButtonElement>("[data-bib-row-primary]");
    if (button) { button.focus({ preventScroll: true }); pendingFocus.current = null; }
  });
  useEffect(() => {
    const node = viewport.current;
    if (!node) return;
    const keydown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement;
      if (target !== node && !target.closest("[data-bib-row-primary]")) return;
      const focused = target.closest<HTMLElement>("[data-article-id]")?.dataset.articleId ?? selectedId;
      const index = focused ? articles.findIndex(article => article.id === focused) : -1;
      if (event.code === "Space" && (event.ctrlKey || event.metaKey || target === node)) { if (focused) { event.preventDefault(); onToggle(focused); } return; }
      let next: number;
      switch (event.key) {
        case "ArrowDown": next = Math.min(articles.length - 1, index + 1); break;
        case "ArrowUp": next = Math.max(0, index - 1); break;
        case "Home": next = 0; break;
        case "End": next = articles.length - 1; break;
        case "PageDown": next = Math.min(articles.length - 1, index + Math.max(1, Math.floor(node.clientHeight / estimatedHeight))); break;
        case "PageUp": next = Math.max(0, index - Math.max(1, Math.floor(node.clientHeight / estimatedHeight))); break;
        default: return;
      }
      if (!articles[next]) return;
      event.preventDefault(); pendingFocus.current = articles[next].id;
      const top = layout.offsets[next], bottom = layout.offsets[next + 1];
      if (top < node.scrollTop) node.scrollTop = top;
      else if (bottom > node.scrollTop + node.clientHeight) node.scrollTop = bottom - node.clientHeight;
      setWindow({ top: node.scrollTop, height: node.clientHeight || 600 }); onNavigate(articles[next].id);
    };
    node.addEventListener("keydown", keydown);
    return () => node.removeEventListener("keydown", keydown);
  }, [articles, estimatedHeight, layout, onNavigate, onToggle, selectedId, viewport]);
  const indices = new Set(Array.from({length:Math.max(0,end-start)},(_,index)=>start+index));
  const focusedIndex = focusedId ? layout.positions.get(focusedId) : undefined;
  if (focusedIndex !== undefined) indices.add(focusedIndex);
  return <div ref={container} onFocusCapture={event => setFocusedId((event.target as HTMLElement).closest<HTMLElement>("[data-article-id]")?.dataset.articleId ?? null)} onBlurCapture={event => {if (!event.currentTarget.contains(event.relatedTarget)) setFocusedId(null);}} role="list" data-loaded-count={articles.length} aria-label="Literature results" style={{ position: "relative", height: layout.offsets.at(-1) ?? 0, overflowAnchor: "none" }}>
    {[...indices].sort((a,b)=>a-b).map(index => {const article=articles[index];return <MeasuredRow key={article.id} id={article.id} top={layout.offsets[index]} index={index} total={total} onMeasure={measure}>{children(article)}</MeasuredRow>;})}
  </div>;
}
