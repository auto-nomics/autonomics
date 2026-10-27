import { build } from 'esbuild';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
const root = path.dirname(fileURLToPath(import.meta.url));
await build({
  entryPoints: [path.join(root, 'frontend.mjs')],
  outfile: path.join(root, '../../apps/gui/src/zotero-sdt.generated.js'),
  bundle: true, format: 'esm', target: 'es2022',
  banner: { js: '/*! @license Zotero Reader components: AGPL-3.0. Copyright Corporation for Digital Scholarship.\n * Source revisions and notices: tools/zotero-extractor/UPSTREAM.json and THIRD_PARTY.md.\n * Regenerate: node tools/zotero-extractor/build-frontend.mjs */' },
  plugins: [{
    name: 'autonomics-host',
    setup(build) {
      build.onResolve({ filter: /structured-document-text\// }, args => {
        const relative = args.path.split('structured-document-text/')[1];
        return { path: path.join(root, 'vendor/document-worker/structured-document-text', relative + (path.extname(relative) ? '' : '.js')) };
      });
      // The mapper's optional getSortIndex method uses only these two host
      // utilities. Keeping the host adapter avoids importing Zotero's viewer.
      build.onResolve({ filter: /\/pdf\/selection$/ }, () => ({ path: 'sort-index-host', namespace: 'host' }));
      build.onLoad({ filter: /.*/, namespace: 'host' }, () => ({ contents: `
        export function getSortIndexRect(position) {
          const rects = position.rects || [];
          return rects.length ? [Math.min(...rects.map(r => r[0])), Math.min(...rects.map(r => r[1])), Math.max(...rects.map(r => r[2])), Math.max(...rects.map(r => r[3]))] : null;
        }
        export function formatSortIndex(pageIndex, offset, top) {
          return [pageIndex.toString().slice(0, 5).padStart(5, '0'), offset.toString().slice(0, 6).padStart(6, '0'), Math.floor(top).toString().slice(0, 5).padStart(5, '0')].join('|');
        }
      ` }));
    },
  }],
});
