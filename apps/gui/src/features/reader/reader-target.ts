export type ReaderWorkspaceOptions = {
  host?: HTMLElement;
  onReference?: (selection: { text: string; page?: number; start?: number; end?: number }) => void;
  /** Fires once when the reader is destroyed for any reason (own close
   *  button, superseded by another reader). Hosts that survive the destroy
   *  use it to react — the workbench editor closes its tab. */
  onClosed?: () => void;
};
export type ReaderSearchTarget = { query?: string; page?: number | null; annotationId?: string | null };
export function readerBase(articleId: string, attachmentKey?: string | null) {
  const base = `/bib/articles/${encodeURIComponent(articleId)}`;
  return attachmentKey ? `${base}/attachments/${encodeURIComponent(attachmentKey)}` : base;
}
export function annotationPath(articleId: string, attachmentKey?: string | null) { return `${readerBase(articleId, attachmentKey)}/annotations`; }
export function structurePath(articleId: string, attachmentKey?: string | null) { return `${readerBase(articleId, attachmentKey)}/fulltext/structured`; }
export function readerLocationId(articleId: string, attachmentKey?: string | null) { return attachmentKey ? `${articleId}.${attachmentKey}` : articleId; }

let generation = 0;
/** Live reader sessions: token → destroy handle. By default opening a
 *  reader supersedes (closes) the live ones — the legacy single-window
 *  behavior tests and modal hosts rely on. The workbench reader editor
 *  opts out (supersede: false) so its tabs coexist and keep their state. */
const liveSessions = new Map<number, () => void>();
export function beginReaderSession(options: { supersede?: boolean } = {}) {
  if (options.supersede ?? true) {
    for (const close of [...liveSessions.values()]) close();
  }
  const token = ++generation;
  // Seed the map immediately — the session is live from birth; the real
  // destroy handle lands via registerReaderSession.
  liveSessions.set(token, () => {});
  return token;
}
export function isReaderSession(token: number) { return liveSessions.has(token); }
export function registerReaderSession(token: number, close: () => void) { if (!liveSessions.has(token)) return false; liveSessions.set(token, close); return true; }
export function releaseReaderSession(token: number) { liveSessions.delete(token); }

/** Search with original UTF-16 offsets despite expanding lowercase characters
 * and whitespace folding. Evidence uses lexical boundaries; ordinary Ctrl+F
 * remains a substring search. */
export function findTextRanges(text: string, query: string, wordBoundary = false, limit = 1000): Array<{ start: number; end: number }> {
  const needle = query.trim().replace(/\s+/g, " ").toLowerCase();
  if (!needle) return [];
  let folded = "", offset = 0;
  const map: Array<{ start: number; end: number }> = [];
  for (const char of text) {
    const start = offset; offset += char.length;
    const value = /\s/u.test(char) ? " " : char.toLowerCase();
    if (value === " " && folded.endsWith(" ")) { map[map.length - 1].end = offset; continue; }
    folded += value;
    for (let unit = 0; unit < value.length; unit++) map.push({ start, end: offset });
  }
  const word = (char: string | undefined) => Boolean(char && /[\p{L}\p{N}]/u.test(char) && !/\p{Script=Han}/u.test(char));
  const edges = Array.from(needle);
  const ranges = [];
  for (let from = 0; ranges.length < limit;) {
    const at = folded.indexOf(needle, from);
    if (at < 0) break;
    const end = at + needle.length;
    const before = Array.from(folded.slice(Math.max(0, at - 2), at)).at(-1);
    const after = Array.from(folded.slice(end, end + 2))[0];
    const partialFold = (at > 0 && map[at - 1].start === map[at].start) || (end < map.length && map[end - 1].end === map[end].end);
    if (!wordBoundary || (!partialFold && !((word(edges[0]) && word(before)) || (word(edges.at(-1)) && word(after))))) ranges.push({ start: map[at].start, end: map[end - 1].end });
    from = end;
  }
  return ranges;
}

export function trapReaderFocus(event: KeyboardEvent, root: HTMLElement) {
  if (event.key !== "Tab") return;
  const targets = [...root.querySelectorAll<HTMLElement>('button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),a[href],[tabindex="0"]')].filter(node => node.getClientRects().length && !node.closest('[hidden]'));
  const first=targets[0],last=targets.at(-1);
  if (!first) {event.preventDefault();root.focus();return;}
  if (event.shiftKey && (document.activeElement===first || document.activeElement===root)) {event.preventDefault();last?.focus();}
  else if (!event.shiftKey && document.activeElement===last) {event.preventDefault();first.focus();}
}
export function restoreReaderFocus(preferred: Element | null) {
  if (preferred instanceof HTMLElement && preferred.isConnected) preferred.focus({preventScroll:true});
  else document.querySelector<HTMLElement>('[aria-label="Literature results viewport"]')?.focus({preventScroll:true});
}
