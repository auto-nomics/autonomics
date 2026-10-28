import type { StructuredDocumentText, ContentBlockNode } from '../../../tools/zotero-extractor/vendor/document-worker/structured-document-text/schema';
export type { StructuredDocumentText, ContentBlockNode };
export type SDTPosition = { start: number[]; end: number[] };
export type PDFPosition = { pageIndex: number; rects: number[][]; nextPageRects?: number[][] };
export function renderSDT(structure: StructuredDocumentText, doc: Document, options?: { renderSourceCrops?: boolean }): HTMLElement;
export function getBlockNodeByRef(content: ContentBlockNode[], ref: number[]): ContentBlockNode | null;
export class PDFPositionMapper {
  constructor(structure: StructuredDocumentText);
  sdtToSourcePosition(position: SDTPosition): PDFPosition | null;
  sourceToSDTPosition(position: PDFPosition): SDTPosition | null;
}
