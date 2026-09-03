/**
 * PDF 悬浮翻译的预翻译结果缓存（v2 — 按字符范围查找）
 *
 * 早期版本用 cacheKey + 字符串模糊匹配查段落，命中率受 MinerU/WASM 文本差异影响极大。
 * 现在改为：每次页面加载时跑反向定位（charAlign::locateTextInPage），把每个 MinerU
 * 段落 / 句子的"字符范围"算出来存到 entry 上；hover 时按 charIndex 直接字符范围
 * 查找，零字符串匹配。
 *
 * 第一层 key 必须包含 docId：不同论文在同一 pageIdx 上的预翻译是不一样的。
 */

import { locateTextInPage, locateTextInPageFrom, locateByPrefix } from './charAlign';
import { splitIntoSentences } from './paragraphDetection';
import type { SentencePair } from '../types';
export type { SentencePair };

/**
 * 带字符范围对齐信息的句子（SentencePair 扩展）。
 *
 * `charStart/charEnd` 是反向定位到 PDFium 字符流的结果；
 * `undefined` 表示该句定位失败（MinerU 文本与字符流差异过大），hover 时跳过。
 */
export interface AlignedSentence extends SentencePair {
  charStart?: number;
  charEnd?: number;
}

/** 翻译缓存条目（对应 ParagraphTranslation 表里的一行） */
export interface TranslationCacheEntry {
  /** 整段译文（fallback 用） */
  translation: string;
  /** 句子级对齐数组（带 charStart/charEnd） */
  sentences: AlignedSentence[];
  /** MinerU 段落原文（block.text）—— 用于整段反向定位，比单句定位稳定 */
  originalText: string;
  /** 段落级字符范围（反向定位结果；undefined = 该段定位失败） */
  charStart?: number;
  charEnd?: number;
  /** 段落边界框（PDF points，与 PDFium device 坐标同系；来自 PdfBlock）。
   *  坐标命中主键：鼠标 deviceX/deviceY 落在 [x0,x1)×[y0,y1) 内即命中该段。
   *  undefined = 无 blockId 关联或 bbox 全 0（无效）。 */
  bboxX0?: number;
  bboxY0?: number;
  bboxX1?: number;
  bboxY1?: number;
}

/**
 * 构造桶键（外层 Map 的 key）
 *
 * 把 docId 和 pageIdx 拼成 `docId@pageIdx`。docId 是 DocumentManagerPlugin
 * 分配的文档 id（与 paperId 一一对应），切论文时会换。
 */
function bucketKey(docId: string, pageIdx: number): string {
  return `${docId}@${pageIdx}`;
}

/**
 * 从文本生成缓存键（后端 upsert 去重用）
 *
 * 算法: trim → 折叠多空白为单空格 → lowercase → 前 200 个 Unicode 标量
 * 必须与后端 `TranslationRepo::get_cache_key` 完全一致。
 *
 * 注意：**仅用于与后端 cacheKey 对齐去重，不再用于 hover 查找**。
 * hover 查找走 charAlign 反向定位后的字符范围（findEntryByCharRange）。
 *
 * 注意按 **Unicode 标量** 而非 UTF-16 code unit 切片：Rust 端 `chars().take(200)`
 * 走的是标量，emoji / 罕用 CJK 扩展字符在 UTF-16 里是代理对（2 个 code unit），
 * 直接 `slice(0, 200)` 可能劈开代理对，两端就会漂移。
 */
export function getCacheKey(text: string): string {
  const folded = text.trim().replace(/\s+/g, ' ').toLowerCase();
  return Array.from(folded).slice(0, 200).join('');
}

/** 按文档×页分桶的缓存：外层 bucketKey → 内层 cacheKey → entry */
const pageBuckets = new Map<string, Map<string, TranslationCacheEntry>>();

/**
 * 整页批量写入预翻译结果
 *
 * @param docId 文档 ID（来自 DocumentManagerPlugin）
 * @param pageIdx 0-based 页码
 * @param entries 该页所有 ParagraphTranslation（key 是 cacheKey）
 */
export function setPageEntries(
  docId: string,
  pageIdx: number,
  entries: Map<string, TranslationCacheEntry>,
): void {
  pageBuckets.set(bucketKey(docId, pageIdx), entries);
}

/**
 * 对当前页所有 entry 跑反向定位，把 charStart/charEnd 写到 entry 和 sentences 上
 *
 * 必须在 getPageGeometry 拿到 geo.text（pageText）之后、用户 hover 之前调用。
 * 调用幂等：重复调用会重新跑反向定位并覆盖（用于切回已访问页时刷新）。
 *
 * 算法（两层）：
 * 1. **段落级反向定位**：用 entry.originalText（MinerU block.text 整段）在 pageText
 *    里精确匹配/anchor 兜底。整段长（数百字符），特征强，定位最稳。
 * 2. **句子级范围重切**：在段落定位成功后，对 pageText 的段落切片跑 splitIntoSentences，
 *    得到 PDF 字符流上的精确句子边界；如果切句数 == entry.sentences 数，按顺序
 *    把范围对应到 entry.sentences[i]。
 *
 * 为什么不直接用 LLM 输出的 sentence.original 做单句反向定位？
 * - LLM 经常把前一句的句号粘到本句开头（". Currently, ..."）
 * - LLM 经常把本句末尾的上标引用（"treatment15" 的 "15"）丢给下一句
 * - 这些 LLM 改写让 sentence.original 在 pageText 里"看起来偏移"，单句定位范围漂移
 * - 用 MinerU 整段定位 + pageText 切句绕开 LLM 改写，得到 PDF 字符流的精确边界
 *
 * 当切句数不一致时（LLM 合并/拆分了句子）fall back 到单句独立定位（容错但可能漂移）。
 *
 * @param docId 文档 ID
 * @param pageIdx 0-based 页码
 * @param pageText PDFium 抽取的全页原始字符流（geo.text，未剥离 PDF 标记）
 * @returns 该页段落定位成功的 entry 数（诊断用）
 */
/**
 * 句子级顺序贪心对齐（替代"单句独立定位"的 fallback）
 *
 * 把 LLM 的 sentences 按阅读顺序在 pageText 里连续定位：从前一句的命中末尾
 * 继续找下一句，保证范围连续且不重叠。即便 LLM 改写了某个句子的 original
 * （如去掉上标引用、合并/拆分句子），只要该句的原文仍能在 pageText 中找到，
 * 就能定位到正确位置，不会因"独立 indexOf"匹配到错误副本。
 *
 * 定位失败的句子（original 在 pageText 中不存在）不写入 charStart/charEnd，
 * hover 时由段落级范围兜底。
 */
function alignSentencesSequential(
  sentences: AlignedSentence[],
  pageText: string,
): void {
  let fromNormPos = 0;
  for (const sent of sentences) {
    if (!sent.original) continue;
    // 三层定位（逐级放宽，保证句子起点尽可能被定位到）：
    // 1. 顺序精确匹配：从上一句末尾开始 indexOf 整句（范围连续不重叠）
    // 2. anchor 容错：首尾各 15 字符定位（不受 fromNormPos 限制，救回越过起点的句子）
    // 3. 纯前缀定位：只用前 20 字符定位起点（救回 LLM 拼接句——后半内容在 pageText
    //    不连续，anchor 的 suffix 会匹配到错误位置，但起点稳定）
    let located: { start: number; end: number } | null = null;
    let nextPos = fromNormPos;
    const h1 = locateTextInPageFrom(pageText, sent.original, fromNormPos);
    if (h1) {
      located = { start: h1.start, end: h1.end };
      nextPos = h1.nextNormPos;
    } else {
      const h2 = locateTextInPage(pageText, sent.original);
      if (h2) {
        located = { start: h2.start, end: h2.end };
        nextPos = h2.end;
      } else {
        const h3 = locateByPrefix(pageText, sent.original, fromNormPos);
        if (h3) {
          located = { start: h3.start, end: h3.end };
          nextPos = h3.nextNormPos;
        }
      }
    }
    if (located) {
      sent.charStart = located.start;
      sent.charEnd = located.end;
      fromNormPos = nextPos;
    }
    // 三层都失败：不写入 charStart/charEnd，hover 时由段落级范围兜底
  }
}

export function alignPageEntries(
  docId: string,
  pageIdx: number,
  pageText: string,
): number {
  const page = pageBuckets.get(bucketKey(docId, pageIdx));
  if (!page || page.size === 0) return 0;

  // DEV 模式：记录失败的 originalText 前 60 字符，便于诊断 MinerU/PDFium 文本差异
  const failedSnippets: string[] = [];

  let alignedCount = 0;
  for (const [, entry] of page) {
    if (!entry.originalText) continue;

    // 1. 段落级反向定位：用 MinerU 整段原文在 pageText 里定位
    const blockHit = locateTextInPage(pageText, entry.originalText);
    if (!blockHit) {
      // 段落定位失败：用顺序贪心对齐逐句定位（连续不重叠，避免独立定位的范围错位）
      alignSentencesSequential(entry.sentences, pageText);
      if (import.meta.env?.DEV) {
        failedSnippets.push(entry.originalText.slice(0, 60));
      }
      continue;
    }

    entry.charStart = blockHit.start;
    entry.charEnd = blockHit.end;
    alignedCount++;

    // 2. 在 pageText 的段落范围内重新切句（splitIntoSentences 在 PDF 字符流上工作）
    //    切句规则按句末标点（. ! ? 。 ！ ？），与 LLM 改写无关。
    const blockSlice = pageText.slice(blockHit.start, blockHit.end);
    const splitSentences = splitIntoSentences(blockSlice);

    // 3. 句子对应：切句数 == LLM sentences 数 → 按顺序映射范围
    if (splitSentences.length === entry.sentences.length) {
      for (let i = 0; i < entry.sentences.length; i++) {
        entry.sentences[i].charStart = blockHit.start + splitSentences[i].start;
        entry.sentences[i].charEnd = blockHit.start + splitSentences[i].end;
      }
    } else {
      // 切句数不一致（LLM 合并/拆分了句子，或 splitIntoSentences 漏切/误切）：
      // 用顺序贪心对齐替代独立单句定位，保证句子范围连续不重叠，
      // 避免 LLM 改写 sentence.original 时独立 indexOf 匹配到错误副本导致范围错位。
      alignSentencesSequential(entry.sentences, pageText);
    }
  }

  if (failedSnippets.length > 0 && import.meta.env?.DEV) {
    console.warn(
      `[translationCache] page ${pageIdx} 段落反向定位失败的 entry（前 ${failedSnippets.length} 条，前 60 字符）:`,
      failedSnippets,
    );
  }
  return alignedCount;
}

/**
 * 按 hover 命中的字符位置查找翻译条目和对应句子
 *
 * 先在桶里找段落级范围命中的 entry；再在 entry 内找句子级范围命中的句子。
 * 句子级优先（更精确），段落级 fallback（用于 hover 在两句之间的空白、或某句定位失败时）。
 *
 * @returns `{ entry, sentenceIndex }` 或 `undefined`（未命中）
 *          sentenceIndex = -1 表示命中段落但未命中具体句子（用 entry.translation fallback）
 */
export function findEntryByCharRange(
  docId: string,
  pageIdx: number,
  charIndex: number,
): { entry: TranslationCacheEntry; sentenceIndex: number } | undefined {
  const page = pageBuckets.get(bucketKey(docId, pageIdx));
  if (!page || page.size === 0) return undefined;


  // 第一遍：找段落级命中的 entry（可能多个 entry 有重叠范围，取第一个）
  let hitEntry: TranslationCacheEntry | undefined;
  for (const [, entry] of page) {
    if (
      entry.charStart !== undefined &&
      entry.charEnd !== undefined &&
      charIndex >= entry.charStart &&
      charIndex < entry.charEnd
    ) {
      hitEntry = entry;
      break;
    }
  }

  // 第二遍：在所有 entry 的句子里找精确命中（句子级范围比段落级更可靠，
  // 即使段落级定位失败也可能句子级成功）
  for (const [, entry] of page) {
    for (let i = 0; i < entry.sentences.length; i++) {
      const sent = entry.sentences[i];
      if (
        sent.charStart !== undefined &&
        sent.charEnd !== undefined &&
        charIndex >= sent.charStart &&
        charIndex < sent.charEnd
      ) {
        return { entry, sentenceIndex: i };
      }
    }
  }

  // 句子级未命中但有段落级命中：返回段落 + sentenceIndex = -1（让 UI fallback）
  if (hitEntry) {
    return { entry: hitEntry, sentenceIndex: -1 };
  }
  return undefined;
}

/**
 * 按 hover 的鼠标设备坐标做几何包含查询（主命中路径）
 *
 * 坐标系：MinerU bbox 与 PDFium device 坐标同系（均左上原点、Y 向下、PDF points≈72DPI device px）。
 * 鼠标 deviceX/deviceY 落在 entry.bbox 矩形内即命中该段——**零字符串匹配**，
 * 不受 LLM 改写原文 / 跨页拼接 / 标签差异影响。
 *
 * 返回命中的段落级 entry（sentenceIndex 恒为 -1，句子级高亮由调用方在命中后用
 * entry 内的 char range 再细分；若该段句子级 char range 缺失，则用 bbox 整段高亮）。
 *
 * @returns 命中的 entry（sentenceIndex=-1）或 undefined（鼠标不在任何预翻译段落内）
 */
export function findEntryByBBox(
  docId: string,
  pageIdx: number,
  deviceX: number,
  deviceY: number,
): { entry: TranslationCacheEntry; sentenceIndex: number } | undefined {
  const page = pageBuckets.get(bucketKey(docId, pageIdx));
  if (!page || page.size === 0) return undefined;

  for (const [, entry] of page) {
    const x0 = entry.bboxX0;
    const y0 = entry.bboxY0;
    const x1 = entry.bboxX1;
    const y1 = entry.bboxY1;
    // bbox 缺失或全 0（无效，通常 blockId 为 null 的按需翻译）→ 跳过
    if (x0 === undefined || y0 === undefined || x1 === undefined || y1 === undefined) continue;
    if (x0 === 0 && y0 === 0 && x1 === 0 && y1 === 0) continue;
    // 几何包含（含左/上边界，不含右/下边界，避免相邻段落共边重复命中）
    if (deviceX >= x0 && deviceX < x1 && deviceY >= y0 && deviceY < y1) {
      return { entry, sentenceIndex: -1 };
    }
  }
  return undefined;
}

/**
 * 切页时清理：只保留当前页 ±1 页（避免内存累积）
 *
 * 只清理同一 docId 下的远端页；其他 docId 的桶交给 evictDocument 切换论文时清。
 */
export function evictNonAdjacentPages(docId: string, currentPage: number): void {
  const prefix = `${docId}@`;
  for (const key of pageBuckets.keys()) {
    if (!key.startsWith(prefix)) continue;
    const pageIdx = Number(key.slice(prefix.length));
    if (Math.abs(pageIdx - currentPage) > 1) {
      pageBuckets.delete(key);
    }
  }
}

/**
 * 切换文档时清掉该文档的所有桶
 *
 * 关闭 PDF / 切到另一篇论文时调用，避免老论文的预翻译常驻内存。
 */
export function evictDocument(docId: string): void {
  const prefix = `${docId}@`;
  for (const key of pageBuckets.keys()) {
    if (key.startsWith(prefix)) pageBuckets.delete(key);
  }
}
