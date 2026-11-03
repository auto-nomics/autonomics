// Thin host adapter. PDF decoding, ML segmentation and SDT assembly are Zotero's
// unmodified implementations pinned in UPSTREAM.json.
//
// Image export is a host extension layered on top of the SDT structure: every
// non-auxiliary image block's anchor rect is rendered to PNG through pdf.js
// (same technique as upstream's renderer.js, with @napi-rs/canvas), named
// after the PNG content hash and referenced from the node via `imageRef`.
// Set ZOTERO_EXTRACT_IMAGES=0 to disable the rendering pass entirely.
import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { format } from 'node:util';
import { createCanvas } from '@napi-rs/canvas';

const root = path.dirname(fileURLToPath(import.meta.url));
const upstream = path.join(root, 'vendor/document-worker');
// stdout is exclusively the single JSON response consumed by the Rust host.
for (const name of ['log', 'info', 'warn', 'debug']) {
  console[name] = (...args) => process.stderr.write(format(...args) + '\n');
}
await import('./vendor/document-worker/scripts/pdfjs-setup.js');
const { getDocument } = await import('./vendor/document-worker/pdf.js/src/pdf.js');
const { getStructure } = await import('./vendor/document-worker/src/pdf/index.js');
const { getBlockClusterRuntime } = await import('./vendor/document-worker/src/pdf/structure/model/block-seg/clusterer/runtime.js');
const { getBlockSegClassifierRuntime } = await import('./vendor/document-worker/src/pdf/structure/model/block-seg/classifier/runtime.js');
const { getFulltextFromStructuredText } = await import('./vendor/document-worker/structured-document-text/src/fulltext.js');

async function dataProvider(name) {
  let filename;
  if (name.startsWith('block-seg/')) filename = path.join(upstream, 'src/pdf/structure/model', name);
  else if (name === 'onnx/ort-wasm-simd-threaded.wasm') filename = path.join(root, 'node_modules/onnxruntime-web/dist/ort-wasm-simd-threaded.wasm');
  else if (name.startsWith('wasm/')) {
    const decoder = name.includes('jbig2') ? 'jbig2' : name.includes('qcms') ? 'qcms' : name.includes('brotli') ? 'brotli' : 'openjpeg';
    filename = path.join(upstream, 'pdf.js/external', decoder, path.basename(name));
  }
  else filename = path.join(upstream, 'pdf.js/external', name);
  const bytes = await readFile(filename);
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
}

export async function extractPDF(bytes, { onProgress } = {}) {
  // Initialize the actual upstream models before page extraction: Zotero has a
  // per-page degraded fallback, which must not hide missing installation assets.
  const runtime = () => dataProvider('onnx/ort-wasm-simd-threaded.wasm');
  const model = name => dataProvider(name);
  await getBlockClusterRuntime(runtime, model);
  await getBlockSegClassifierRuntime(runtime, model);
  const data = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
  const structure = await getStructure(data, undefined, dataProvider, {
    sourceHash: createHash('md5').update(bytes).digest('hex'),
    onProgress,
  });
  const text = getFulltextFromStructuredText(structure, structure.catalog.pages.map((_, index) => index));
  const images = await exportFigureImages(bytes, structure, dataProvider);
  return { text, format: 'plain', extractor: 'zotero-sdt', structure, images };
}

// --- Figure image export (host extension) -----------------------------------
//
// The ML segmentation already identifies image blocks; this pass turns each
// block's anchor rect into PNG pixels. Rendering (instead of lifting embedded
// image objects) handles CMYK, masks and strip-split rasters for free. The
// caps mirror the Rust side's extraction limits so a pathological document
// cannot balloon stdout.

const FIGURE_SCALE = 4; // matches upstream renderer.js
const MAX_CANVAS_PIXELS = 16777216; // 16 megapixels
const MAX_FIGURES = 64;
const MAX_FIGURE_BYTES = 15 * 1024 * 1024;
const MAX_TOTAL_IMAGE_BYTES = 32 * 1024 * 1024; // pre-base64 budget

async function exportFigureImages(bytes, structure, dataProvider) {
  if (process.env.ZOTERO_EXTRACT_IMAGES === '0') {
    return [];
  }
  const blocks = [];
  collectFigureBlocks(structure.content, blocks);
  if (!blocks.length) {
    return [];
  }
  const images = [];
  let total = 0;
  let pdfDocument;
  try {
    pdfDocument = await openRenderableDocument(bytes, dataProvider);
    for (const block of blocks) {
      if (images.length >= MAX_FIGURES) {
        process.stderr.write(`zotero-sdt: more than ${MAX_FIGURES} figure blocks; skipping the rest\n`);
        break;
      }
      const rect = pickFigureRect(block.anchor.pageRects);
      if (!rect) {
        continue;
      }
      try {
        const page = await pdfDocument.getPage(rect.pageIndex + 1);
        const png = await renderRect(page, rect.box);
        if (!png || png.byteLength > MAX_FIGURE_BYTES || total + png.byteLength > MAX_TOTAL_IMAGE_BYTES) {
          process.stderr.write(`zotero-sdt: figure at page ${rect.pageIndex} skipped (size budget)\n`);
          continue;
        }
        const name = `z-${createHash('sha256').update(png).digest('hex').slice(0, 16)}.png`;
        if (!images.some(image => image.name === name)) {
          total += png.byteLength;
          images.push({ name, data: Buffer.from(png).toString('base64') });
        }
        block.imageRef = name;
      }
      catch (error) {
        // A single failed render never fails the extraction.
        process.stderr.write(`zotero-sdt: figure render failed at page ${rect.pageIndex}: ${error.stack || error}\n`);
      }
    }
  }
  catch (error) {
    process.stderr.write(`zotero-sdt: figure export unavailable: ${error.stack || error}\n`);
    for (const block of blocks) {
      delete block.imageRef;
    }
    return [];
  }
  finally {
    await pdfDocument?.destroy?.();
  }
  return images;
}

function collectFigureBlocks(node, out) {
  if (Array.isArray(node)) {
    for (const child of node) collectFigureBlocks(child, out);
    return;
  }
  if (!node || typeof node !== 'object') {
    return;
  }
  // 'excluded' blocks are running headers/footers and page furniture; image
  // blocks are normally 'auxiliary' (outside the body text flow) and must
  // still be exported.
  if (node.type === 'image'
    && node.flowClass !== 'excluded'
    && Array.isArray(node.anchor?.pageRects)
    && node.anchor.pageRects.length) {
    out.push(node);
  }
  if (Array.isArray(node.content)) {
    collectFigureBlocks(node.content, out);
  }
}

// pageRects entries are [pageIndex, x1, y1, x2, y2] in user space. Rects of
// one block may span pages (rare); render the page carrying the most area
// and merge that page's rects into one bounding box.
function pickFigureRect(pageRects) {
  const byPage = new Map();
  for (const rect of pageRects) {
    if (!Array.isArray(rect) || rect.length < 5) {
      continue;
    }
    const [pageIndex, x1, y1, x2, y2] = rect;
    const width = Math.abs(x2 - x1);
    const height = Math.abs(y2 - y1);
    if (!Number.isInteger(pageIndex) || pageIndex < 0 || !(width > 0) || !(height > 0)) {
      continue;
    }
    const entry = byPage.get(pageIndex) || { area: 0, boxes: [] };
    entry.area += width * height;
    entry.boxes.push([Math.min(x1, x2), Math.min(y1, y2), Math.max(x1, x2), Math.max(y1, y2)]);
    byPage.set(pageIndex, entry);
  }
  let best;
  for (const [pageIndex, entry] of byPage) {
    if (!best || entry.area > best.area) {
      const box = [
        Math.min(...entry.boxes.map(b => b[0])),
        Math.min(...entry.boxes.map(b => b[1])),
        Math.max(...entry.boxes.map(b => b[2])),
        Math.max(...entry.boxes.map(b => b[3]))
      ];
      best = { pageIndex, box };
    }
  }
  return best;
}

async function openRenderableDocument(bytes, dataProvider) {
  // Mirrors vendor renderer.js getRenderablePdfDocument: same factory wiring,
  // buffer copy (getDocument may transfer it), no vendor canvas runtime.
  const options = {
    data: new Uint8Array(bytes).buffer,
    cMapUrl: null,
    cMapPacked: true,
    standardFontDataUrl: null,
    wasmUrl: null,
    useWorkerFetch: false,
    disableFontFace: false
  };
  if (dataProvider) {
    options.CMapReaderFactory = function () {
      this.fetch = async ({ name }) => {
        const raw = await dataProvider('cmaps/' + name + '.bcmap');
        return { cMapData: raw, isCompressed: true };
      };
    };
    options.StandardFontDataFactory = function () {
      this.fetch = async ({ filename }) => dataProvider('standard_fonts/' + filename);
    };
    options.WasmFactory = function () {
      this.fetch = async ({ filename }) => dataProvider('wasm/' + filename);
    };
  }
  return getDocument(options).promise;
}

async function renderRect(page, rect) {
  const view = page.view;
  const clamped = [
    Math.max(rect[0], view[0]),
    Math.max(rect[1], view[1]),
    Math.min(rect[2], view[2]),
    Math.min(rect[3], view[3])
  ];
  const width = clamped[2] - clamped[0];
  const height = clamped[3] - clamped[1];
  if (!(width > 0) || !(height > 0)) {
    return null;
  }
  const scale = Math.min(FIGURE_SCALE, Math.sqrt(MAX_CANVAS_PIXELS / (width * height)));
  const viewport = page.getViewport({ scale });
  const [ax, ay] = viewport.convertToViewportPoint(clamped[0], clamped[1]);
  const [bx, by] = viewport.convertToViewportPoint(clamped[2], clamped[3]);
  const left = Math.min(ax, bx);
  const top = Math.min(ay, by);
  const canvasWidth = Math.ceil(Math.max(ax, bx) - left);
  const canvasHeight = Math.ceil(Math.max(ay, by) - top);
  if (!canvasWidth || !canvasHeight) {
    return null;
  }
  const offsetViewport = page.getViewport({ scale, offsetX: -left, offsetY: -top });
  const canvas = createCanvas(canvasWidth, canvasHeight);
  const ctx = canvas.getContext('2d', { alpha: false });
  ctx.fillStyle = '#ffffff';
  ctx.fillRect(0, 0, canvasWidth, canvasHeight);
  await page.render({ canvasContext: ctx, viewport: offsetViewport }).promise;
  return canvas.toBuffer('image/png');
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const chunks = [];
    for await (const chunk of process.stdin) chunks.push(chunk);
    const result = await extractPDF(Buffer.concat(chunks), {
      onProgress: progress => process.stderr.write(`zotero-sdt progress=${Math.round(progress)}\n`),
    });
    process.stdout.write(JSON.stringify(result));
  } catch (error) {
    process.stderr.write(`zotero-sdt: ${error.stack || error}\n`);
    process.exitCode = 1;
  }
}
