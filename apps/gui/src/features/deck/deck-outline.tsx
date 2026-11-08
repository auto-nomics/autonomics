import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import type { OutlineEntry, SlideSpec } from "./deck-api";

/** Compatibility for older servers: show a flat page list. */
export function outlineOrPages(outline: OutlineEntry[] | undefined, slides: SlideSpec[]): OutlineEntry[] {
  return outline ?? slides.map(slide => ({ slide: slide.index + 1, title: slide.title || `第 ${slide.index + 1} 页`, level: slide.level ?? 1, depth: 0, parent: null, isSection: false, sectionTitle: slide.title || "", sectionPath: [], hidden: !!slide.frontmatter.hideInToc }));
}

export function DeckOutline({ entries, currentSlide, onNavigate }: { entries: OutlineEntry[]; currentSlide: number; onNavigate: (slide: number) => void }): React.ReactNode {
  const [collapsed, setCollapsed] = useState<Set<number>>(() => new Set());
  const bySlide = useMemo(() => new Map(entries.map(entry => [entry.slide, entry])), [entries]);
  // Skip hidden entries when calculating the visible tree, retaining their
  // descendants beneath the nearest visible ancestor.
  const visibleParent = (entry: OutlineEntry): number | null => {
    let parent = entry.parent;
    while (parent !== null && bySlide.get(parent)?.hidden) parent = bySlide.get(parent)?.parent ?? null;
    return parent;
  };
  const visible = entries.filter(entry => !entry.hidden);
  const parentMap = new Map(visible.map(entry => [entry.slide, visibleParent(entry)]));
  const parents = new Set(Array.from(parentMap.values()).filter((n): n is number => n !== null));
  useEffect(() => {
    // Moving to a page via keyboard or the section bar reveals its path.
    const ancestors: number[] = [];
    let parent = bySlide.get(currentSlide)?.parent ?? null;
    while (parent !== null) { ancestors.push(parent); parent = bySlide.get(parent)?.parent ?? null; }
    setCollapsed(previous => {
      if (!ancestors.some(no => previous.has(no))) return previous;
      const next = new Set(previous); ancestors.forEach(no => next.delete(no)); return next;
    });
    // Do not reopen user-collapsed sections on each background refresh.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentSlide]);
  if (!visible.length) return <p className="px-2 py-3 text-xs text-muted-foreground">没有可显示的页面。</p>;
  return <div role="list" aria-label="页面目录">{visible.map(entry => {
    let parent = parentMap.get(entry.slide) ?? null;
    let depth = 0;
    let concealed = false;
    while (parent !== null) { depth++; if (collapsed.has(parent)) concealed = true; parent = parentMap.get(parent) ?? null; }
    if (concealed) return null;
    const hasChildren = parents.has(entry.slide);
    const isCollapsed = collapsed.has(entry.slide);
    return <div role="listitem" key={entry.slide} className={`deck-outline-row ${currentSlide === entry.slide ? "deck-toc-item--active" : ""}`} style={{ paddingLeft: `${4 + Math.min(depth, 6) * 13}px` }}>
      {hasChildren ? <button type="button" className="deck-outline-toggle" aria-label={`${isCollapsed ? "展开" : "折叠"} ${entry.title}`} aria-expanded={!isCollapsed} onClick={() => setCollapsed(previous => { const next = new Set(previous); if (next.has(entry.slide)) next.delete(entry.slide); else next.add(entry.slide); return next; })}>{isCollapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}</button> : <span className="deck-outline-toggle" aria-hidden="true" />}
      <button type="button" className="deck-outline-link" aria-current={currentSlide === entry.slide ? "page" : undefined} title={entry.title} onClick={() => onNavigate(entry.slide)}><span className="deck-toc-no">{entry.slide}</span><span className="deck-toc-label">{entry.title}</span></button>
    </div>;
  })}</div>;
}
