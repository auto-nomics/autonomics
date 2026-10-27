import type { HighlightRange } from "./bib-search-model";

/** The API uses Unicode code point offsets, so emoji and non-BMP characters
 * must be split with Array.from rather than JavaScript string slicing. */
export function SearchSnippet({ text, ranges = [] }: { text: string; ranges?: HighlightRange[] }) {
  const chars = Array.from(text);
  const parts = [];
  let at = 0;
  for (const range of ranges) {
    if (!Number.isInteger(range.start) || !Number.isInteger(range.end) || range.start < at || range.end > chars.length || range.end <= range.start) continue;
    if (range.start > at) parts.push(<span key={`text-${at}`}>{chars.slice(at, range.start).join("")}</span>);
    parts.push(<mark key={`match-${range.start}`} className="rounded-sm bg-amber-200/70 text-inherit dark:bg-amber-700/60">{chars.slice(range.start, range.end).join("")}</mark>);
    at = range.end;
  }
  if (at < chars.length) parts.push(<span key={`text-${at}`}>{chars.slice(at).join("")}</span>);
  return <>{parts}</>;
}
