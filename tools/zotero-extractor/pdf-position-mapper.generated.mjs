/*! @license Zotero Reader components: AGPL-3.0. Copyright Corporation for Digital Scholarship.
 * Source revisions and notices: tools/zotero-extractor/UPSTREAM.json and THIRD_PARTY.md.
 * Regenerate: node tools/zotero-extractor/build-anchor.mjs */

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

// tools/zotero-extractor/vendor/document-worker/structured-document-text/src/pdf/utils.js
function isWhitespaceChar(ch) {
  return ch === " " || ch === "\n" || ch === "	";
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
  PDFPositionMapper
};
