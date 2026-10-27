/** Semantic offsets supplement PDF rectangles: rectangles can be paragraph-sized
 * when an extractor lacks character maps, so the conversion is not invertible. */
export type ReadingTextRange = {
  source_hash: string;
  start: number[];
  end: number[];
  quote: string;
};

export const normalizeQuote = (text: string) => text.replace(/\s+/g, ' ').trim();

/** Index semantic text once per document, not on every scroll/annotation paint.
 * Normalized characters retain their original DOM offsets, including whitespace
 * spanning adjacent inline nodes. This also repairs older rectangle-only marks. */
export class TextQuoteIndex {
  private nodes: Text[] = [];
  private starts: number[] = [];
  private offsets: number[] = [];
  private nodeStarts = new WeakMap<Node, number>();
  private text = '';

  constructor(article: HTMLElement) {
    const walker = document.createTreeWalker(article, NodeFilter.SHOW_TEXT);
    let rawOffset = 0;
    const characters: string[] = [];
    while (walker.nextNode()) {
      const node = walker.currentNode as Text;
      if (!node.parentElement?.closest('[data-text-index]')) continue;
      this.nodes.push(node); this.starts.push(rawOffset); this.nodeStarts.set(node, rawOffset);
      for (let offset = 0; offset < node.length; offset++) {
        const character = /\s/.test(node.data[offset]) ? ' ' : node.data[offset];
        if (character !== ' ' || characters.at(-1) !== ' ') {
          characters.push(character); this.offsets.push(rawOffset + offset);
        }
      }
      rawOffset += node.length;
    }
    this.text = characters.join('');
  }

  private point(offset: number) {
    let low = 0, high = this.starts.length;
    while (low < high) {
      const middle = (low + high) >>> 1;
      if (this.starts[middle] <= offset) low = middle + 1;
      else high = middle;
    }
    const index = Math.max(0, low - 1), node = this.nodes[index];
    return node && { node, offset: Math.min(node.length, offset - this.starts[index]) };
  }

  find(quote: string, near: Range): Range | null {
    quote = normalizeQuote(quote);
    if (!quote) return null;
    const start = this.nodeStarts.get(near.startContainer), end = this.nodeStarts.get(near.endContainer);
    if (start === undefined || end === undefined) return null;
    const nearStart = start + near.startOffset, nearEnd = end + near.endOffset;
    let best = -1, distance = Infinity;
    for (let match = this.text.indexOf(quote); match !== -1; match = this.text.indexOf(quote, match + 1)) {
      const left = this.offsets[match], right = this.offsets[match + quote.length - 1] + 1;
      // Use geometry to distinguish identical quotes on different pages/blocks.
      // Never pick unrelated text elsewhere in the document just by its words.
      if (right <= nearStart || left >= nearEnd) continue;
      const delta = Math.abs(left - nearStart) + Math.abs(right - nearEnd);
      if (delta < distance) { best = match; distance = delta; }
    }
    if (best < 0) return null;
    const first = this.point(this.offsets[best]), last = this.point(this.offsets[best + quote.length - 1] + 1);
    if (!first || !last) return null;
    const range = document.createRange();
    range.setStart(first.node, first.offset); range.setEnd(last.node, last.offset);
    return range;
  }
}
