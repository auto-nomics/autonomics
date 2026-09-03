/**
 * charAlign — MinerU 段落文本到 PDFium 字符流的反向定位
 *
 * 背景：
 * - 后端用 MinerU 抽取段落文本（每段一个 PdfBlock），喂 LLM 翻译存到 DB
 * - 前端 WASM PDFium 抽取字符流（getPageGeometry + getTextSlices），用于 hover 命中
 * - 两条管线独立，文本边界天然不一致（空白、连字符断行、换行符差异、
 *   MinerU 全角 vs PDFium 半角括号、LLM 对 sentence.original 的轻微改写）
 *
 * 本模块在 hover 链路之前做一次性反向定位：把每个 MinerU 段落 / 句子原文
 * 在 PDFium 字符流里找到对应位置 [start, end)，存到内存缓存。hover 时按
 * 字符范围直接查，零字符串匹配。
 *
 * 算法（两层降级）：
 * 1. **精确子串匹配**：归一化后 indexOf，O(N+M)，命中即返回
 * 2. **前后锚点容错**：target 中段可能有字符差异（如全角/半角括号、LLM 改写），
 *    但首尾通常稳定。取前 15 + 后 15 字符做 anchor，分别匹配；任一失败则整体失败
 *
 * 归一化规则：
 * - 跳过所有空白
 * - 处理 "epider-\nmal" 断行（跳过 hyphen + 后续空白）
 * - 小写化
 * - 保留数字和标点（两边一致时直接匹配；不一致时由 anchor fallback 兜底）
 *
 * @module charAlign
 */

/** 锚点长度：target 首尾各取这么多个归一化字符做容错匹配 */
const ANCHOR_LEN = 15;

/** target 太短拒绝匹配（归一化后字符数） */
const MIN_TARGET_LEN = 6;

/**
 * PDF "不可见"标记字符集合 —— 与 pdfium-viewer 的 PdfUnwantedTextMarkers 保持一致。
 *
 * 这些字符由 PDF 作者工具插入，渲染时不占视觉位置（零宽），
 * 但 PDFium 字符流里仍计入它们。getTextSlices 会剥离，但 getPageGeometry
 * 输出的 geo.text 保留（为了与 runs[].charStart 索引对齐）。
 *
 * 在 normalize 时跳过这些字符，让 MinerU 文本（通常不含这些标记）能匹配
 * 到原始 PDFium 字符流（含这些标记）。
 */
const PDF_UNWANTED_MARKERS = new Set([
  '­', // Soft Hyphen
  '​', // Zero Width Space
  '⁠', // Word Joiner
  '﻿', // BOM / Zero Width No-Break Space
  '￾', // Non-character
  '￿', // Non-character
]);

/**
 * 归一化结果：归一化后的字符序列 + 每个归一化字符在原文里的下标
 *
 * 用于在归一化空间找到匹配后，反推回原文坐标。
 */
interface NormalizedText {
  text: string;
  /** origPositions[i] = 第 i 个归一化字符在原文中的下标 */
  origPositions: number[];
}

/**
 * 把文本归一化：去所有空白 + 处理 hyphen-newline 断行 + 跳过 PDF 不可见标记 + 小写
 */
function normalize(s: string): NormalizedText {
  // 先剥离 HTML 标签符号（保留标签内的视觉文本）：
  // MinerU 输出的 block.text / LLM 的 sentence.original 可能含 <sup>...</sup> 等
  // 标签。这些标签的尖括号字符（< > /）在 PDFium 字符流里不存在，但标签内的
  // 文本（如 <sup>fi</sup> 里的 fi、<sup>13</sup> 里的 13、<sup>INTRODUCTION</sup>
  // 里的 INTRODUCTION）通常作为视觉字符存在于 PDFium 字符流中。
  // 因此剥离标签符号、保留内容（与后端 strip_html_tags 策略一致），让两端对齐。
  // 注意：不能用"跳过整个 <...> 块"——那会丢弃 <sup>fi</sup> 的 fi，导致
  // "Speci<sup>fi</sup>cally" 归一化成 "specically" 与 pageText 的 "specifically" 不匹配。
  const stripped = s.replace(/<[^>]*>/g, '');
  const chars: string[] = [];
  const positions: number[] = [];
  let i = 0;
  const len = stripped.length;
  while (i < len) {
    const ch = stripped[i];
    // 跳过 PDF 不可见标记字符（软连字符、零宽空格等）—— 它们不出现在 MinerU 文本里，
    // 但可能出现在 geo.text 中，必须跳过才能让两端对齐。
    if (PDF_UNWANTED_MARKERS.has(ch)) {
      i++;
      continue;
    }
    // 跳过所有连字符（-）：
    // - PDF 断行的 "epider-\nm" → "epiderm"（原 hyphen+空白 规则）
    // - LLM 改写的复合词连字符：LLM 常把 anticancer 写成 anti-cancer、
    //   monoubiquitination 写成 mono-ubiquitination，单字内连字符差异会让整句
    //   精确匹配失败。跳过所有连字符让 anti-cancer ≡ anticancer；pageText 侧的
    //   连字符同样被跳过，两端一致，不影响定位精度。
    if (ch === '-') {
      i++;
      while (i < len && /\s/.test(stripped[i])) i++;
      continue;
    }
    if (/\s/.test(ch)) {
      i++;
      continue;
    }
    chars.push(ch.toLowerCase());
    positions.push(i);
    i++;
  }
  return { text: chars.join(''), origPositions: positions };
}

/**
 * 把归一化位置反映射回原文位置
 *
 * @param norm 归一化结果（含 origPositions 数组）
 * @param normStart 归一化空间里的起始下标（含）
 * @param normEnd 归一化空间里的结束下标（不含）
 * @returns 原文位置的 `{ start, end }`，下标越界返回 null
 */
function mapBack(
  norm: NormalizedText,
  normStart: number,
  normEnd: number,
): { start: number; end: number } | null {
  if (normStart < 0 || normEnd <= normStart) return null;
  const lastIdx = normEnd - 1;
  if (lastIdx >= norm.origPositions.length) return null;
  const startOrig = norm.origPositions[normStart];
  const endOrig = norm.origPositions[lastIdx];
  if (startOrig === undefined || endOrig === undefined) return null;
  return { start: startOrig, end: endOrig + 1 };
}

/**
 * 在 pageText 里定位 target 的字符范围
 *
 * 算法两层降级：
 * 1. 精确子串匹配（indexOf）
 * 2. 失败时用首尾 ANCHOR_LEN 字符做 anchor，分别匹配后合并范围
 *
 * @param pageText PDFium 抽取的全页字符流（含空白、换行）
 * @param target MinerU 段落或句子的原文
 * @returns `{ start, end }` 在 pageText 中的字符下标范围（end 不含）；未找到返回 null
 */
export function locateTextInPage(
  pageText: string,
  target: string,
): { start: number; end: number } | null {
  const normPage = normalize(pageText);
  const normTarget = normalize(target).text;

  // 太短的 target 容易误匹配，直接拒绝
  if (normTarget.length < MIN_TARGET_LEN) return null;

  // 第一层：精确子串匹配
  const exactIdx = normPage.text.indexOf(normTarget);
  if (exactIdx >= 0) {
    const r = mapBack(normPage, exactIdx, exactIdx + normTarget.length);
    if (r) return r;
  }

  // 第二层：首尾 anchor 容错
  // target 中段可能有少量字符差异（全角/半角括号、LLM 改写），
  // 但首尾通常稳定。anchor 太短会误匹配，所以最少要 6 字符。
  const anchorLen = Math.min(ANCHOR_LEN, Math.floor(normTarget.length / 4));
  if (anchorLen < MIN_TARGET_LEN) return null;

  const prefix = normTarget.slice(0, anchorLen);
  const suffix = normTarget.slice(-anchorLen);

  const prefixIdx = normPage.text.indexOf(prefix);
  if (prefixIdx < 0) return null;

  // suffix 必须出现在 prefix 之后（避免倒序匹配）
  const suffixIdx = normPage.text.indexOf(suffix, prefixIdx + prefix.length);
  if (suffixIdx < 0) return null;

  return mapBack(normPage, prefixIdx, suffixIdx + suffix.length);
}

/**
 * 顺序贪心定位：从 pageText 的 fromNormPos（归一化坐标）之后查找 target，
 * 返回首个匹配位置；未找到返回 null
 *
 * 与 locateTextInPage 的区别：
 * - 不做 anchor fallback（要求 target 在 pageText 中真实存在，避免范围漂移）
 * - 从指定归一化位置开始查找（用于顺序对齐：前一句命中后，下一句从其末尾继续找）
 *
 * 用于 alignPageEntries 的句子级顺序对齐：把多个 LLM sentence 按阅读顺序
 * 在 pageText 里连续定位，避免"独立单句定位"因 LLM 改写导致范围错位/重叠。
 *
 * @param pageText PDFium 全页字符流
 * @param target 待定位的句子原文
 * @param fromNormPos 归一化空间里的搜索起点（0=从头）
 * @returns `{ start, end, nextNormPos }` 原文坐标范围 + 下一次搜索的归一化起点；未找到 null
 */
export function locateTextInPageFrom(
  pageText: string,
  target: string,
  fromNormPos = 0,
): { start: number; end: number; nextNormPos: number } | null {
  const normPage = normalize(pageText);
  const normTarget = normalize(target).text;
  if (normTarget.length < MIN_TARGET_LEN) return null;

  const idx = normPage.text.indexOf(normTarget, fromNormPos);
  if (idx < 0) return null;
  const r = mapBack(normPage, idx, idx + normTarget.length);
  if (!r) return null;
  return { start: r.start, end: r.end, nextNormPos: idx + normTarget.length };
}

/**
 * 纯前缀定位：用 target 的前 20 个归一化字符在 pageText 里定位起点
 *
 * 用于句子级最终兜底。当 LLM 把不连续的内容拼成一句（sentence.original 长达数百
 * 字符但前半和后半来自不同位置）时，整句匹配和 anchor（prefix+suffix）都会失败，
 * 因为 suffix 会匹配到错误位置。但句子起点（如 "Here, we specifically"）几乎总是
 * 稳定的，用纯前缀定位起点即可让 hover 命中。
 *
 * end = 起点 + target 归一化长度（覆盖整句，即便后半不精确；hover 高亮容忍）。
 */
/**
 * 纯前缀定位 + 最长连续匹配（LCP）确定 end
 *
 * 用于句子级最终兜底。当 LLM 把不连续的内容拼成一句（sentence.original 长达数百
 * 字符但前半和后半来自不同 PDF block / 跨页）时，整句匹配和 anchor 都会失败。
 * 但句子起点（如 "Here, we specifically"）几乎总是稳定的：
 * 1. 用前 20 字符定位起点
 * 2. 从起点逐字符比对 target 与 pageText，直到首个差异 → LCP（最长连续匹配）
 *    end 停在实际匹配的末尾，不会越过翻页接缝把后面的页脚/机构信息包进高亮。
 */
export function locateByPrefix(
  pageText: string,
  target: string,
  fromNormPos = 0,
): { start: number; end: number; nextNormPos: number } | null {
  const normPage = normalize(pageText);
  const normTarget = normalize(target).text;
  if (normTarget.length < MIN_TARGET_LEN) return null;
  const prefixLen = Math.min(20, normTarget.length);
  const prefix = normTarget.slice(0, prefixLen);
  const prefixIdx = normPage.text.indexOf(prefix, fromNormPos);
  if (prefixIdx < 0) return null;
  // LCP：从 prefixIdx 逐字符比对，直到首个差异
  let lcp = 0;
  for (let i = 0; i < normTarget.length && prefixIdx + i < normPage.text.length; i++) {
    if (normTarget[i] === normPage.text[prefixIdx + i]) lcp++;
    else break;
  }
  if (lcp < MIN_TARGET_LEN) return null;
  const r = mapBack(normPage, prefixIdx, prefixIdx + lcp);
  if (!r) return null;
  return { start: r.start, end: r.end, nextNormPos: prefixIdx + prefixLen };
}

