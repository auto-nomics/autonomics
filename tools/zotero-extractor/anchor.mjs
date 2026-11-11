// Resolve a verified quote to the same PDF geometry used by the reader.
import { PDFPositionMapper } from './pdf-position-mapper.generated.mjs';

export function anchorQuote(structure, quote) {
  const chars = [], points = [];
  const append = (text, ref) => {
    for (let i = 0; i < text.length; i++) {
      const c = /\s/u.test(text[i]) ? ' ' : text[i];
      if (c === ' ' && chars.at(-1) === ' ') continue;
      chars.push(c); points.push(ref ? { ref, offset: i } : null);
    }
  };
  const walk = (node, ref) => {
    if (typeof node?.text === 'string') { append(node.text, ref); return; }
    if (!Array.isArray(node?.content)) return;
    const blocks = node.content.some(child => typeof child?.text !== 'string');
    node.content.forEach((child, i) => { if (blocks && i) append(' ', null); walk(child, [...ref, i]); });
  };
  walk(structure, []);
  const text = chars.join('');
  const needle = quote.replace(/\s+/gu, ' ').trim();
  const at = text.indexOf(needle);
  if (!needle || at < 0) throw Error('quote cannot be mapped to PDF text; re-extract the PDF or choose a shorter verbatim quote');
  if (text.indexOf(needle, at + 1) >= 0) throw Error('quote is ambiguous in PDF text; include more surrounding text');
  const first = points[at], last = points[at + needle.length - 1];
  if (!first || !last) throw Error('quote boundaries have no PDF text position');
  const start = [...first.ref, first.offset], end = [...last.ref, last.offset + 1];
  const mapper = new PDFPositionMapper(structure);
  const position = mapper.sdtToSourcePosition({ start, end });
  if (!position?.rects?.length) throw Error('PDF source map has no geometry for this quote');
  if (position.nextPageRects?.length) throw Error('quote spans multiple PDF pages; split it into one note per page');
  const view = structure.catalog.pages[position.pageIndex]?.viewRect;
  if (!view || view.length !== 4) throw Error('PDF page dimensions are missing');
  const [x0, y0, x1, y1] = view;
  const rects = position.rects.map(([l, b, r, t], i) => ({
    left: (l-x0)/(x1-x0), top: (b-y0)/(y1-y0), width: (r-l)/(x1-x0), height: (t-b)/(y1-y0), space: 'pdf',
    ...(i === 0 ? { text_range: { source_hash: structure.metadata.source.hash, start, end, quote } } : {}),
  }));
  if (rects.some(r => ![r.left,r.top,r.width,r.height].every(Number.isFinite) || r.width <= 0 || r.height <= 0 || r.left < 0 || r.top < 0 || r.left+r.width > 1.001 || r.top+r.height > 1.001)) throw Error('PDF source map produced invalid rectangles');
  return { page: position.pageIndex + 1, rects };
}

if (typeof process !== 'undefined' && process.argv[1] && new URL(import.meta.url).pathname === process.argv[1]) {
  try {
    const chunks = [];
    for await (const chunk of process.stdin) chunks.push(chunk);
    const { structure, quote } = JSON.parse(Buffer.concat(chunks));
    process.stdout.write(JSON.stringify(anchorQuote(structure, quote)));
  } catch (error) {
    process.stderr.write(String(error.message));
    process.exitCode = 1;
  }
}
