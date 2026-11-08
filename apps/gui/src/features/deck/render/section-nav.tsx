import type { OutlineEntry } from "../deck-api";

/** The same navigation markup is used in interactive and print views. */
export function SectionNav({ outline, slide, onNavigate }: { outline?: OutlineEntry[]; slide: number; onNavigate?: (slide: number) => void }): React.ReactNode {
  if (!outline?.length) return null;
  const sections = outline.filter(entry => entry.isSection && !entry.hidden && entry.sectionPath.length === 1);
  if (!sections.length) return null;
  const current = outline.find(entry => entry.slide === slide);
  const path = current?.sectionPath ?? [];
  const bySlide = new Map(outline.map(entry => [entry.slide, entry]));
  const nested = path.slice(1).map(no => bySlide.get(no)).filter((entry): entry is OutlineEntry => !!entry);
  return <nav className="deck-section-nav" aria-label="章节导航" data-has-current={path.length > 0 ? "true" : "false"} data-current-section={path[0] ?? ""} data-current-subsection={path.length > 1 ? path[path.length - 1] : ""}>
    <ol className="deck-section-nav-list">{sections.map((section, index) => {
      const active = path[0] === section.slide;
      return <li key={section.slide} className="deck-section-nav-item" data-active={active ? "true" : "false"} data-section-slide={section.slide} data-section-index={index + 1}>
        <button type="button" className="deck-section-nav-link" data-active={active ? "true" : "false"} aria-current={active ? "location" : undefined} disabled={!onNavigate} onClick={() => onNavigate?.(section.slide)}><span className="deck-section-nav-number">{String(index + 1).padStart(2, "0")}</span><span className="deck-section-nav-title">{section.sectionTitle}</span></button>
      </li>;
    })}</ol>
    {nested.length > 0 && <div className="deck-section-breadcrumb" aria-label="当前子章节">{nested.map(entry => <span className="deck-section-breadcrumb-item" key={entry.slide}>{entry.sectionTitle}</span>)}</div>}
  </nav>;
}
