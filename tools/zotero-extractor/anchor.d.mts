export function anchorQuote(structure: unknown, quote: string): {
  page: number;
  rects: Array<{ left: number; top: number; width: number; height: number; space: 'pdf'; text_range?: { source_hash: string; start: number[]; end: number[]; quote: string } }>;
};
