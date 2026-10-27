/*! @license Zotero Reader components: AGPL-3.0. Copyright Corporation for Digital Scholarship.
 * Source revisions and notices: tools/zotero-extractor/UPSTREAM.json and THIRD_PARTY.md.
 * Regenerate: node tools/zotero-extractor/build-frontend.mjs */

// tools/zotero-extractor/vendor/document-worker/structured-document-text/src/pdf/constants.js
var HEADER_LAST_IS_SOFT_HYPHEN = 1 << 0;
var HEADER_AXIS_DIR_SHIFT = 1;
var HEADER_DIR_RTL = 1 << 3;
var isVertical = (axisDir) => axisDir === 1 || axisDir === 3;

// tools/zotero-extractor/vendor/document-worker/structured-document-text/src/pdf/decode.js
function parseTextMap(textMap) {
  if (typeof textMap !== "string") {
    return [];
  }
  try {
    const parsed = JSON.parse(textMap);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}
function reconstructCharPositions(run) {
  if (!run || run.length < 6) return [];
  const [header, pageIndex, minX, minY, maxX, maxY, ...widths] = run;
  const axisDir = header >> HEADER_AXIS_DIR_SHIFT & 3;
  const vertical = isVertical(axisDir);
  const start = vertical ? minY : minX;
  const end = vertical ? maxY : maxX;
  if (widths.length === 0) {
    return [{ x1: start, x2: end }];
  }
  const positions = [];
  let pos = start;
  for (const w of widths) {
    if (Array.isArray(w)) {
      const [delta, width] = w;
      pos += delta;
      positions.push({ x1: pos, x2: pos + width });
      pos += width;
    } else {
      positions.push({ x1: pos, x2: pos + w });
      pos += w;
    }
  }
  return positions;
}
function buildRunData(runs) {
  const data = [];
  for (const run of runs) {
    if (!Array.isArray(run) || run.length < 6) {
      continue;
    }
    const [header, pageIndex, minX, minY, maxX, maxY] = run;
    const axisDir = header >> HEADER_AXIS_DIR_SHIFT & 3;
    const vertical = isVertical(axisDir);
    const positions = reconstructCharPositions(run);
    if (header & 1 << 0) {
      positions.pop();
    }
    for (const pos of positions) {
      if (!pos || !Number.isFinite(pos.x1) || !Number.isFinite(pos.x2)) {
        continue;
      }
      const rect = vertical ? [minX, pos.x1, maxX, pos.x2] : [pos.x1, minY, pos.x2, maxY];
      data.push({ rect, pageIndex, vertical });
    }
  }
  return data;
}

// tools/zotero-extractor/vendor/document-worker/structured-document-text/src/range.js
function splitContentPoint(point, content) {
  if (!Array.isArray(content)) {
    throw new TypeError("splitContentPoint requires content");
  }
  if (!point) {
    return { ref: null, offset: void 0 };
  }
  if (!Array.isArray(point)) {
    return { ref: null, offset: void 0 };
  }
  let ref = [];
  let node = null;
  for (let i = 0; i < point.length; i++) {
    let value = point[i];
    if (!Number.isInteger(value)) {
      return { ref: null, offset: void 0 };
    }
    if (isTextNode(node) && i === point.length - 1) {
      return { ref, offset: value };
    }
    ref.push(value);
    node = ref.length === 1 ? content[value] : Array.isArray(node?.content) ? node.content[value] : null;
  }
  return { ref, offset: void 0 };
}
function splitContentRange(range, content) {
  return {
    start: splitContentPoint(range?.[0], content),
    end: splitContentPoint(range?.[1], content)
  };
}
function getContentRangeBlockSpan(contentRange, topLevelBlockCount) {
  if (!isContentRange(contentRange) || !Number.isInteger(topLevelBlockCount) || topLevelBlockCount < 0) {
    return null;
  }
  const start = contentRange[0];
  const end = contentRange[1];
  const startIndex = getBoundaryTopLevelIndex(start, topLevelBlockCount);
  if (startIndex === null) {
    return null;
  }
  if (sameBoundary(start, end)) {
    return { startIndex, endIndexExclusive: startIndex };
  }
  const endIndexExclusive = getBoundaryEndIndexExclusive(end, topLevelBlockCount);
  if (endIndexExclusive === null) {
    return null;
  }
  return {
    startIndex,
    endIndexExclusive: Math.max(startIndex, endIndexExclusive)
  };
}
function isContentRange(value) {
  return Array.isArray(value) && value.length === 2 && isContentBoundary(value[0]) && isContentBoundary(value[1]);
}
function isContentBoundary(value) {
  return Array.isArray(value) && value.length > 0 && value.every((index) => Number.isInteger(index) && index >= 0);
}
function isTextNode(node) {
  return !!node && typeof node === "object" && typeof node.text === "string";
}
function compareRefs(a, b) {
  const length = Math.min(a.length, b.length);
  for (let i = 0; i < length; i++) {
    if (a[i] !== b[i]) {
      return a[i] - b[i];
    }
  }
  return a.length - b.length;
}
function refKey(ref) {
  return Array.isArray(ref) ? ref.join(",") : "";
}
function isLeafBlock(node) {
  if (!node || typeof node.text === "string") {
    return false;
  }
  if (!Array.isArray(node.content) || node.content.length === 0) {
    return true;
  }
  return !node.content.some((child) => child && typeof child.text !== "string");
}
function walkContentRangeLeafBlocks(content, range, callback) {
  if (!Array.isArray(content) || typeof callback !== "function") {
    return;
  }
  const span = getContentRangeBlockSpan(range, content.length);
  if (!span || span.startIndex >= span.endIndexExclusive) {
    return;
  }
  let parts;
  try {
    parts = splitContentRange(range, content);
  } catch (_) {
    return;
  }
  for (let i = span.startIndex; i < span.endIndexExclusive; i++) {
    const block = content[i];
    if (!block || typeof block.text === "string") {
      continue;
    }
    walkLeafBlocks(block, [i], range, parts, callback);
  }
}
function walkLeafBlocks(node, ref, range, parts, callback) {
  if (!node || typeof node.text === "string") {
    return;
  }
  if (isLeafBlock(node)) {
    const leafEnd = getLeafEndBoundary(ref);
    if (compareRefs(leafEnd, range[0]) <= 0 || compareRefs(ref, range[1]) >= 0) {
      return;
    }
    callback({
      block: node,
      ref,
      startPoint: parts.start,
      endPoint: parts.end
    });
    return;
  }
  for (let i = 0; i < node.content.length; i++) {
    const child = node.content[i];
    if (child && typeof child.text !== "string") {
      walkLeafBlocks(child, [...ref, i], range, parts, callback);
    }
  }
}
function getLeafEndBoundary(ref) {
  const end = [...ref];
  end[end.length - 1]++;
  return end;
}
function sameBoundary(a, b) {
  return Array.isArray(a) && Array.isArray(b) && a.length === b.length && a.every((value, index) => value === b[index]);
}
function getBoundaryTopLevelIndex(boundary, topLevelBlockCount) {
  if (!isContentBoundary(boundary)) {
    return null;
  }
  const index = boundary[0];
  if (index > topLevelBlockCount) {
    return null;
  }
  return index;
}
function getBoundaryEndIndexExclusive(boundary, topLevelBlockCount) {
  const index = getBoundaryTopLevelIndex(boundary, topLevelBlockCount);
  if (index === null) {
    return null;
  }
  if (index === topLevelBlockCount) {
    return topLevelBlockCount;
  }
  return boundary.length === 1 ? index : index + 1;
}

// tools/zotero-extractor/vendor/document-worker/structured-document-text/src/parts.js
function getNodeByRef(structure, ref) {
  if (!Array.isArray(ref)) {
    return null;
  }
  let node = { content: structure?.content };
  for (const index of ref) {
    if (!Number.isInteger(index) || !Array.isArray(node?.content)) {
      return null;
    }
    node = node.content[index];
    if (!node || typeof node !== "object") {
      return null;
    }
  }
  return node;
}
function getFirstTextNode(node) {
  if (!node || typeof node !== "object") {
    return null;
  }
  if (typeof node.text === "string") {
    return node;
  }
  if (!Array.isArray(node.content)) {
    return null;
  }
  for (const child of node.content) {
    const textNode = getFirstTextNode(child);
    if (textNode) {
      return textNode;
    }
  }
  return null;
}
function getLastTextNode(node) {
  if (!node || typeof node !== "object") {
    return null;
  }
  if (typeof node.text === "string") {
    return node;
  }
  if (!Array.isArray(node.content)) {
    return null;
  }
  for (let i = node.content.length - 1; i >= 0; i--) {
    const textNode = getLastTextNode(node.content[i]);
    if (textNode) {
      return textNode;
    }
  }
  return null;
}
function textNodeEndsWithSoftHyphen(node) {
  const runs = parseTextMap(node?.anchor?.textMap);
  const lastRun = runs.at(-1);
  return Array.isArray(lastRun) && (lastRun[0] & HEADER_LAST_IS_SOFT_HYPHEN) !== 0;
}
function shouldDropHardHyphenAtPartBoundary(prevBlock, nextBlock) {
  const lastNode = getLastTextNode(prevBlock);
  const firstNode = getFirstTextNode(nextBlock);
  const firstChar = firstNode?.text?.charAt(0);
  return typeof lastNode?.text === "string" && lastNode.text.endsWith("-") && firstChar >= "a" && firstChar <= "z";
}
function getPartRoot(structure, ref) {
  let currentRef = Array.isArray(ref) ? [...ref] : null;
  const seen = /* @__PURE__ */ new Set();
  while (currentRef) {
    const key = refKey(currentRef);
    if (seen.has(key)) {
      return currentRef;
    }
    seen.add(key);
    const node = getNodeByRef(structure, currentRef);
    if (!Array.isArray(node?.previousPart)) {
      return currentRef;
    }
    currentRef = [...node.previousPart];
  }
  return null;
}
function getPartChain(structure, ref, options = {}) {
  const include = typeof options.include === "function" ? options.include : null;
  const root = getPartRoot(structure, ref);
  if (!Array.isArray(root)) {
    return [];
  }
  const chain = [];
  const seen = /* @__PURE__ */ new Set();
  let currentRef = [...root];
  while (currentRef) {
    const key = refKey(currentRef);
    if (seen.has(key)) {
      break;
    }
    seen.add(key);
    const node = getNodeByRef(structure, currentRef);
    if (!node) {
      break;
    }
    if (include && !include(currentRef, node)) {
      break;
    }
    chain.push({ ref: currentRef, block: node });
    if (!Array.isArray(node.nextPart)) {
      break;
    }
    currentRef = [...node.nextPart];
  }
  return chain;
}
function getPartBoundarySeparator(prevBlock, nextBlock) {
  const lastNode = getLastTextNode(prevBlock);
  const firstNode = getFirstTextNode(nextBlock);
  if (!lastNode || !firstNode) {
    return "";
  }
  if (/\s$/.test(lastNode.text) || /^\s/.test(firstNode.text)) {
    return "";
  }
  if (textNodeEndsWithSoftHyphen(lastNode)) {
    return "";
  }
  if (/[\p{L}\p{N}]$/u.test(lastNode.text) && /^[\p{L}\p{N}]/u.test(firstNode.text)) {
    return " ";
  }
  return "";
}

// tools/zotero-extractor/vendor/reader/src/dom/sdt/lib/utilities.ts
function isTextNodeArray(content) {
  return content.length > 0 && typeof content[0]?.text === "string";
}

// tools/zotero-extractor/vendor/reader/src/dom/sdt/lib/renderer.ts
function buildHeadingLevels(items, level = 2, headingLevels = /* @__PURE__ */ new Map()) {
  for (let item of items) {
    if (item.ref) {
      let key = refPathToString(item.ref);
      let existingLevel = headingLevels.get(key);
      if (existingLevel === void 0 || level < existingLevel) {
        headingLevels.set(key, level);
      }
    }
    buildHeadingLevels(item.children ?? [], Math.min(level + 1, 6), headingLevels);
  }
  return headingLevels;
}
function renderSDT(structure, doc, { renderSourceCrops = false } = {}) {
  let container = doc.createElement("article");
  container.id = "sdt-content";
  if (structure.metadata.processor.type === "pdf") {
    container.classList.add("sdt-pdf");
  }
  let context = {
    renderSourceCrops,
    headingLevels: buildHeadingLevels(structure.catalog.outline)
  };
  let renderedAsPart = /* @__PURE__ */ new Set();
  for (let [i, block] of structure.content.entries()) {
    if (block.flowClass === "excluded" || renderedAsPart.has(refKey([i]))) {
      continue;
    }
    let el = renderBlock(doc, block, String(i), context);
    if (!el) {
      continue;
    }
    let chain = getMergeableChain(structure, i, renderSourceCrops);
    if (chain) {
      for (let j = 1; j < chain.length; j++) {
        appendChainPart(doc, el, chain[j - 1].block, chain[j].block, chain[j].ref);
        renderedAsPart.add(refKey(chain[j].ref));
      }
    }
    container.append(el);
  }
  return container;
}
function getMergeableChain(structure, index, renderSourceCrops) {
  let block = structure.content[index];
  if (!block.previousPart && !block.nextPart) {
    return null;
  }
  let chain = getPartChain(structure, [index], {
    include: (ref) => ref.length === 1 && structure.content[ref[0]]?.flowClass !== "excluded"
  });
  if (chain.length < 2 || chain[0].ref[0] !== index || !chain.every((part) => !shouldRenderSourceCrop(part.block, renderSourceCrops) && isTextNodeArray(part.block.content))) {
    return null;
  }
  return chain;
}
function shouldRenderSourceCrop(block, renderSourceCrops) {
  return renderSourceCrops && (block.type === "image" || block.type === "math" || block.type === "table" && isTextNodeArray(block.content));
}
function appendChainPart(doc, el, prevBlock, block, ref) {
  if (shouldDropHardHyphenAtPartBoundary(prevBlock, block)) {
    dropTrailingHyphen(el);
  }
  let separator = getPartBoundarySeparator(prevBlock, block);
  if (separator) {
    el.append(doc.createTextNode(separator));
  }
  let refPath = ref.join(".");
  let span = doc.createElement("span");
  span.dataset.refPath = refPath;
  span.id = "sdt-" + refPath;
  span.append(renderTextNodes(doc, block.content));
  el.append(span);
}
function dropTrailingHyphen(el) {
  let walker = el.ownerDocument.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  let lastText = null;
  while (walker.nextNode()) {
    lastText = walker.currentNode;
  }
  if (lastText?.data.endsWith("-")) {
    lastText.data = lastText.data.slice(0, -1);
  }
}
function renderBlock(doc, block, refPath, context) {
  let el;
  switch (block.type) {
    case "paragraph":
      el = doc.createElement("p");
      el.append(renderTextNodes(doc, block.content));
      break;
    case "heading": {
      let level = context.headingLevels.get(refPath) ?? 2;
      el = doc.createElement(`h${level}`);
      el.append(renderTextNodes(doc, block.content));
      break;
    }
    case "math":
      if (context.renderSourceCrops) {
        el = renderSourceCrop(doc, block.content);
        el.classList.add("sdt-math");
      } else {
        el = doc.createElement("div");
        el.className = "sdt-math";
        el.append(renderTextNodes(doc, block.content));
      }
      break;
    case "image": {
      if (context.renderSourceCrops) {
        el = renderSourceCrop(doc, block.content);
      } else {
        el = doc.createElement("figure");
        el.className = "sdt-image";
        let img = doc.createElement("img");
        let altText = block.content.map((node) => node.text).join("").trim();
        if (altText) {
          img.alt = altText;
        }
        el.append(img);
      }
      break;
    }
    case "caption":
      el = doc.createElement("figcaption");
      el.append(renderTextNodes(doc, block.content));
      break;
    case "note":
      el = doc.createElement("aside");
      el.className = "sdt-note";
      el.append(renderTextNodes(doc, block.content));
      break;
    case "preformatted":
      el = doc.createElement("pre");
      el.append(renderTextNodes(doc, block.content));
      break;
    case "blockquote":
      el = renderBlockquote(doc, block, refPath, context);
      break;
    case "list":
      el = renderList(doc, block, refPath, context);
      break;
    case "table":
      el = shouldRenderSourceCrop(block, context.renderSourceCrops) ? renderSourceCrop(doc, block.content) : renderTable(doc, block, refPath, context);
      break;
    default:
      return null;
  }
  el.dataset.refPath = refPath;
  el.id = "sdt-" + refPath;
  if ("reference" in block && block.reference) {
    el.classList.add("sdt-reference");
  }
  if (block.backRefs?.length) {
    el.dataset.backRefs = block.backRefs.map(refPathToString).join(" ");
  }
  return el;
}
function renderSourceCrop(doc, content) {
  let figure = doc.createElement("figure");
  figure.className = "sdt-source-crop";
  let pages = doc.createElement("div");
  pages.className = "sdt-source-crop-pages";
  figure.append(pages);
  let text = doc.createElement("div");
  text.hidden = true;
  text.append(renderTextNodes(doc, content));
  figure.append(text);
  let label = text.textContent?.trim();
  if (label) {
    figure.setAttribute("role", "img");
    figure.setAttribute("aria-label", label);
  }
  return figure;
}
function renderBlockquote(doc, block, refPath, context) {
  let el = doc.createElement("blockquote");
  for (let [i, child] of block.content.entries()) {
    let childEl = renderBlock(doc, child, `${refPath}.${i}`, context);
    if (childEl) {
      el.append(childEl);
    }
  }
  return el;
}
function renderList(doc, block, refPath, context) {
  let el = doc.createElement(block.ordered ? "ol" : "ul");
  if (block.ordered && block.startIndex && block.startIndex !== 1) {
    el.start = block.startIndex;
  }
  for (let [i, item] of block.content.entries()) {
    el.append(renderListItem(doc, item, `${refPath}.${i}`, context));
  }
  return el;
}
function renderListItem(doc, item, refPath, context) {
  let li = doc.createElement("li");
  li.dataset.refPath = refPath;
  li.id = "sdt-" + refPath;
  if (item.reference) {
    li.classList.add("sdt-reference");
  }
  if (item.backRefs?.length) {
    li.dataset.backRefs = item.backRefs.map(refPathToString).join(" ");
  }
  let content = item.content;
  if (!content.length) {
    return li;
  }
  if (isTextNodeArray(content)) {
    li.append(renderTextNodes(doc, content));
  } else {
    for (let [i, child] of content.entries()) {
      let childEl = renderBlock(doc, child, `${refPath}.${i}`, context);
      if (childEl) {
        li.append(childEl);
      }
    }
  }
  return li;
}
function renderTable(doc, block, refPath, context) {
  let table = doc.createElement("table");
  let content = block.content;
  if (!content.length) {
    return table;
  }
  let tbody = doc.createElement("tbody");
  if (isTextNodeArray(content)) {
    let td = doc.createElement("td");
    td.append(renderTextNodes(doc, content));
    let tr = doc.createElement("tr");
    tr.append(td);
    tbody.append(tr);
  } else {
    for (let [i, row] of content.entries()) {
      let tr = doc.createElement("tr");
      tr.dataset.refPath = `${refPath}.${i}`;
      for (let [j, cell] of row.content.entries()) {
        let td = doc.createElement(cell.header ? "th" : "td");
        td.dataset.refPath = `${refPath}.${i}.${j}`;
        if (cell.colspan && cell.colspan > 1) td.colSpan = cell.colspan;
        if (cell.rowspan && cell.rowspan > 1) td.rowSpan = cell.rowspan;
        for (let [k, child] of cell.content.entries()) {
          let childEl = renderBlock(
            doc,
            child,
            `${refPath}.${i}.${j}.${k}`,
            context
          );
          if (childEl) {
            td.append(childEl);
          }
        }
        tr.append(td);
      }
      tbody.append(tr);
    }
  }
  table.append(tbody);
  return table;
}
function renderTextNodes(doc, textNodes) {
  let frag = doc.createDocumentFragment();
  for (let [i, textNode] of textNodes.entries()) {
    frag.append(renderTextNode(doc, textNode, i));
  }
  return frag;
}
function renderTextNode(doc, textNode, index) {
  function wrapIn(node2, tagName) {
    let wrapper = doc.createElement(tagName);
    wrapper.append(node2);
    return wrapper;
  }
  let node = doc.createTextNode(textNode.text);
  let style = textNode.style;
  if (style) {
    if (style.monospace) node = wrapIn(node, "code");
    if (style.sub) node = wrapIn(node, "sub");
    if (style.sup) node = wrapIn(node, "sup");
    if (style.italic) node = wrapIn(node, "em");
    if (style.bold) node = wrapIn(node, "strong");
  }
  if (textNode.target?.url) {
    let a = doc.createElement("a");
    a.href = textNode.target.url;
    a.append(node);
    node = a;
  } else if (textNode.refs?.length) {
    let a = doc.createElement("a");
    a.href = "#sdt-" + refPathToString(textNode.refs[0]);
    a.className = "sdt-ref";
    if (textNode.refs.length > 1) {
      a.dataset.refs = textNode.refs.map(refPathToString).join(" ");
    }
    a.append(node);
    node = a;
  }
  let span = doc.createElement("span");
  span.dataset.textIndex = String(index);
  if (textNode.backRefs?.length) {
    span.dataset.backRefs = textNode.backRefs.map(refPathToString).join(" ");
  }
  span.append(node);
  return span;
}
function refPathToString(ref) {
  return ref.join(".");
}

// tools/zotero-extractor/vendor/document-worker/structured-document-text/src/pdf/utils.js
function isWhitespaceChar(ch) {
  return ch === " " || ch === "\n" || ch === "	";
}

// tools/zotero-extractor/vendor/reader/src/common/defines.js
var READ_ALOUD_BASE_COLOR = "#4072e5";
var READ_ALOUD_ACTIVE_SEGMENT_COLOR = READ_ALOUD_BASE_COLOR + "73";
var READ_ALOUD_ACTIVE_SENTENCE_COLOR = READ_ALOUD_BASE_COLOR + "4d";
var PDF_NOTE_DIMENSIONS = 22;

// host:sort-index-host
function getSortIndexRect(position) {
  const rects = position.rects || [];
  return rects.length ? [Math.min(...rects.map((r) => r[0])), Math.min(...rects.map((r) => r[1])), Math.max(...rects.map((r) => r[2])), Math.max(...rects.map((r) => r[3]))] : null;
}
function formatSortIndex(pageIndex, offset, top) {
  return [pageIndex.toString().slice(0, 5).padStart(5, "0"), offset.toString().slice(0, 6).padStart(6, "0"), Math.floor(top).toString().slice(0, 5).padStart(5, "0")].join("|");
}

// tools/zotero-extractor/vendor/reader/src/common/sdt/position-mapper.ts
function getBlockNodeByRef(content, ref) {
  if (!ref.length) {
    return null;
  }
  let node = content[ref[0]];
  for (let i = 1; i < ref.length; i++) {
    if (!node || !Array.isArray(node.content)) {
      return null;
    }
    node = node.content[ref[i]];
  }
  return node ?? null;
}
function getTextNodeSpans(structure, pos) {
  let spans = [];
  walkContentRangeLeafBlocks(
    structure.content,
    [pos.start, pos.end],
    ({ block, ref, startPoint, endPoint }) => {
      let contentBlock = block;
      let content = contentBlock.content;
      if (!content) {
        return;
      }
      for (let i = 0; i < content.length; i++) {
        let node = content[i];
        if (!node || typeof node.text !== "string") {
          continue;
        }
        let nodeRef = [...ref, i];
        let start = 0;
        let end = node.text.length;
        if (startPoint.ref) {
          let cmp = compareRefs(nodeRef, startPoint.ref);
          if (cmp < 0) {
            continue;
          }
          if (cmp === 0 && Number.isInteger(startPoint.offset)) {
            start = startPoint.offset;
          }
        }
        if (endPoint.ref) {
          let cmp = compareRefs(nodeRef, endPoint.ref);
          if (cmp > 0) {
            continue;
          }
          if (cmp === 0) {
            if (!Number.isInteger(endPoint.offset)) {
              continue;
            }
            end = endPoint.offset;
          }
        }
        if (end <= start) {
          continue;
        }
        spans.push({
          block,
          blockRef: ref,
          node,
          ref: nodeRef,
          start,
          end
        });
      }
    }
  );
  return spans;
}

// tools/zotero-extractor/vendor/reader/src/common/sdt/pdf-position-mapper.ts
var DEFAULT_RUN_DATA_CACHE_WEIGHT = 1e5;
var PDFPositionMapper = class {
  _structure;
  _runDataCache = /* @__PURE__ */ new Map();
  _runDataCacheWeight = 0;
  _maxRunDataCacheWeight;
  constructor(structure, { maxRunDataCacheWeight = DEFAULT_RUN_DATA_CACHE_WEIGHT } = {}) {
    this._structure = structure;
    this._maxRunDataCacheWeight = Math.max(0, maxRunDataCacheWeight);
  }
  sdtToSourcePosition(pos) {
    let spans = getTextNodeSpans(this._structure, pos);
    return this.textNodeSpansToSourcePosition(spans);
  }
  textNodeSpansToSourcePosition(spans) {
    let rectGroupsByPage = /* @__PURE__ */ new Map();
    let addRect = (pageIndex, groupIndex2, rect) => {
      let groups = rectGroupsByPage.get(pageIndex);
      if (!groups) {
        groups = /* @__PURE__ */ new Map();
        rectGroupsByPage.set(pageIndex, groups);
      }
      let rects = groups.get(groupIndex2);
      if (!rects) {
        rects = [];
        groups.set(groupIndex2, rects);
      }
      rects.push(rect);
    };
    let groupIndex = 0;
    let previousBlockIndex = null;
    for (let span of spans) {
      let blockIndex = span.blockRef[0];
      if (previousBlockIndex !== null && blockIndex > previousBlockIndex + 1) {
        groupIndex++;
      }
      previousBlockIndex = blockIndex;
      let runData = this._getRunData(span.node, span.ref);
      if (runData) {
        let runIndex = 0;
        for (let ci = 0; ci < span.node.text.length && runIndex < runData.length; ci++) {
          if (isWhitespaceChar(span.node.text[ci])) {
            continue;
          }
          let run = runData[runIndex++];
          if (ci >= span.start && ci < span.end) {
            addRect(run.pageIndex, groupIndex, run.rect);
          }
        }
      } else {
        for (let pageRect of this._getFallbackPageRects(span)) {
          addRect(pageRect[0], groupIndex, pageRect.slice(1, 5));
        }
      }
    }
    if (!rectGroupsByPage.size) {
      return null;
    }
    let pages = [...rectGroupsByPage.keys()].sort((a, b) => a - b);
    if (pages.length > 2 || pages.length === 2 && pages[1] !== pages[0] + 1) {
      return null;
    }
    let getPageRects = (pageIndex) => [...rectGroupsByPage.get(pageIndex).values()].flatMap((rects) => mergeLineRects(rects));
    let position = {
      pageIndex: pages[0],
      rects: getPageRects(pages[0])
    };
    if (pages.length > 1) {
      position.nextPageRects = getPageRects(pages[1]);
    }
    return position;
  }
  sourceToSDTPosition(position) {
    let pos = position;
    if (!Number.isInteger(pos.pageIndex) || !pos.rects?.length) {
      return null;
    }
    let targets = [{ pageIndex: pos.pageIndex, rects: pos.rects }];
    if (pos.nextPageRects?.length) {
      targets.push({ pageIndex: pos.pageIndex + 1, rects: pos.nextPageRects });
    }
    let start = null;
    let end = null;
    for (let { pageIndex, rects } of targets) {
      let contentRange = this._structure.catalog.pages[pageIndex]?.contentRange;
      if (!contentRange) {
        continue;
      }
      let walkRange = [
        [contentRange[0][0]],
        [Math.min(contentRange[1][0] + 1, this._structure.content.length)]
      ];
      walkContentRangeTextNodes(this._structure.content, walkRange, (node, ref) => {
        let runData = this._getRunData(node, ref);
        if (runData) {
          let runIndex = 0;
          for (let ci = 0; ci < node.text.length && runIndex < runData.length; ci++) {
            if (isWhitespaceChar(node.text[ci])) {
              continue;
            }
            let run = runData[runIndex++];
            if (run.pageIndex !== pageIndex || !rects.some((rect) => quickIntersectRect(rect, run.rect))) {
              continue;
            }
            if (!start) {
              start = [...ref, ci];
            }
            end = [...ref, ci + 1];
          }
        } else {
          let pageRects = node.anchor?.pageRects;
          let intersects = pageRects?.some((pageRect) => pageRect[0] === pageIndex && rects.some((rect) => quickIntersectRect(rect, pageRect.slice(1, 5))));
          if (intersects) {
            if (!start) {
              start = [...ref, 0];
            }
            end = [...ref, node.text.length];
          }
        }
      });
    }
    if (!start || !end) {
      return null;
    }
    return { start, end };
  }
  transformAnnotationPosition(position, type) {
    if (type !== "note") {
      return position;
    }
    let pos = position;
    if (!pos.rects?.length) {
      return position;
    }
    let right = Math.max(...pos.rects.map((rect) => rect[2]));
    let top = Math.max(...pos.rects.map((rect) => rect[3]));
    return {
      pageIndex: pos.pageIndex,
      rects: [[
        right - PDF_NOTE_DIMENSIONS,
        top - PDF_NOTE_DIMENSIONS,
        right,
        top
      ]]
    };
  }
  /**
   * sortIndex for a PDF position, for when no pdf.js page data is available
   * (standalone Reading Mode). Follows the pdf.js-based getSortIndex(), but
   * the character offset counts the SDT's non-whitespace characters on the
   * page, so it can differ slightly from one computed from pdf.js chars.
   */
  getSortIndex(position) {
    let { pageIndex } = position;
    let page = this._structure.catalog.pages[pageIndex];
    let rect = getSortIndexRect(position);
    let top = 0;
    if (page?.viewRect && rect) {
      top = Math.max(0, page.viewRect[3] - page.viewRect[1] - rect[3]);
    }
    let offset = 0;
    let start = this.sourceToSDTPosition(position)?.start;
    let contentRange = page?.contentRange;
    if (start && contentRange) {
      let walkRange = [
        [contentRange[0][0]],
        [Math.min(contentRange[1][0] + 1, this._structure.content.length)]
      ];
      walkContentRangeTextNodes(this._structure.content, walkRange, (node, ref) => {
        let runData = this._getRunData(node, ref);
        let onPage = !runData && !!node.anchor?.pageRects?.some((pageRect) => pageRect[0] === pageIndex);
        let runIndex = 0;
        for (let ci = 0; ci < node.text.length; ci++) {
          if (isWhitespaceChar(node.text[ci])) {
            continue;
          }
          let run = runData?.[runIndex++];
          let point = [...ref, ci];
          if ((run ? run.pageIndex === pageIndex : onPage) && compareRefs(point, start) < 0) {
            offset++;
          }
        }
      });
    }
    return formatSortIndex(pageIndex, offset, top);
  }
  clearCache() {
    this._runDataCache.clear();
    this._runDataCacheWeight = 0;
  }
  _getRunData(node, ref) {
    let key = refKey(ref);
    let cached = this._runDataCache.get(key);
    if (cached) {
      this._runDataCache.delete(key);
      this._runDataCache.set(key, cached);
      return cached.data;
    }
    let textMap = node.anchor?.textMap;
    let runData = textMap ? buildRunData(parseTextMap(textMap)) : null;
    if (runData && !runData.length) {
      runData = null;
    }
    let weight = Math.max(1, runData?.length ?? 0);
    if (weight <= this._maxRunDataCacheWeight) {
      while (this._runDataCacheWeight + weight > this._maxRunDataCacheWeight) {
        let oldest = this._runDataCache.entries().next().value;
        if (!oldest) {
          break;
        }
        this._runDataCache.delete(oldest[0]);
        this._runDataCacheWeight -= oldest[1].weight;
      }
      this._runDataCache.set(key, { data: runData, weight });
      this._runDataCacheWeight += weight;
    }
    return runData;
  }
  _getFallbackPageRects(span) {
    let nodeRects = span.node.anchor?.pageRects;
    if (nodeRects?.length) {
      return nodeRects;
    }
    return span.node.text.trim() ? span.block.anchor?.pageRects ?? [] : [];
  }
};
function walkContentRangeTextNodes(content, range, callback) {
  walkContentRangeLeafBlocks(content, range, ({ block, ref }) => {
    let nodes = block.content;
    if (!nodes) {
      return;
    }
    for (let i = 0; i < nodes.length; i++) {
      if (typeof nodes[i]?.text === "string") {
        callback(nodes[i], [...ref, i]);
      }
    }
  });
}
function quickIntersectRect(r1, r2) {
  return r2[0] < r1[2] && r2[2] > r1[0] && r2[1] < r1[3] && r2[3] > r1[1];
}
function mergeLineRects(rects) {
  let merged = [];
  let current = null;
  for (let rect of rects) {
    if (current && sameLine(current, rect)) {
      current[0] = Math.min(current[0], rect[0]);
      current[1] = Math.min(current[1], rect[1]);
      current[2] = Math.max(current[2], rect[2]);
      current[3] = Math.max(current[3], rect[3]);
    } else {
      current = [...rect];
      merged.push(current);
    }
  }
  return merged;
}
function sameLine(rectA, rectB) {
  let overlap = Math.min(rectA[3], rectB[3]) - Math.max(rectA[1], rectB[1]);
  let minHeight = Math.max(1e-3, Math.min(rectA[3] - rectA[1], rectB[3] - rectB[1]));
  return overlap / minHeight >= 0.6;
}
export {
  PDFPositionMapper,
  getBlockNodeByRef,
  getTextNodeSpans,
  renderSDT
};
