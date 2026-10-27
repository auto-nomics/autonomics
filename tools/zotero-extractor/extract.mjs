// Thin host adapter. PDF decoding, ML segmentation and SDT assembly are Zotero's
// unmodified implementations pinned in UPSTREAM.json.
import { readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { format } from 'node:util';

const root = path.dirname(fileURLToPath(import.meta.url));
const upstream = path.join(root, 'vendor/document-worker');
// stdout is exclusively the single JSON response consumed by the Rust host.
for (const name of ['log', 'info', 'warn', 'debug']) {
  console[name] = (...args) => process.stderr.write(format(...args) + '\n');
}
await import('./vendor/document-worker/scripts/pdfjs-setup.js');
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
  return { text, format: 'plain', extractor: 'zotero-sdt', structure };
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
