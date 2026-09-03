/**
 * paragraphDetection — PDF 文本层的句子切分工具
 *
 * 在 PDFium 字符流（getPageGeometry + getTextSlices 拼出的纯文本）上按
 * 句子边界切分。供 useWasmHoverTranslation 的"hover 命中句子"使用。
 *
 * @module paragraphDetection
 */

/**
 * 句子字符范围（在拼接文本中的位置）
 *
 * 记录句子在传入文本中的起止位置和文本内容，
 * 由 alignPageEntries 用来把 MinerU 段落切片后的句子映射回 PDFium 字符流位置。
 */
export interface SentenceCharRange {
  /** 句子在拼接文本中的起始字符索引 */
  start: number;
  /** 句子在拼接文本中的结束字符索引（不含） */
  end: number;
  /** 句子文本内容 */
  text: string;
}

/**
 * 句子边界识别时需要跳过的缩写表（小写匹配，前必须是词边界）
 *
 * 覆盖学术论文和日常英语中常见的带句点缩写。
 * 与后端 processor.rs:742 的列表保持一致，并补充了公司后缀、章节/页码、学位等。
 */
const SENTENCE_ABBREVIATIONS = [
  // 学位 / 头衔
  'Ph.D', 'M.S', 'B.S', 'A.B', 'M.A', 'M.D',
  'Dr', 'Prof', 'Mr', 'Mrs', 'Ms', 'Sr', 'Jr',
  // 学术缩写
  'et al', 'i.e', 'e.g', 'etc', 'vs', 'cf', 'approx', 'al',
  // 图表 / 章节 / 引用
  'Fig', 'Figs', 'Eq', 'Eqs', 'Tab', 'Tabs', 'Sec', 'Sect', 'Ch',
  'Ref', 'Refs', 'Vol', 'No', 'Nos', 'p', 'pp', 'dept',
  // 公司后缀
  'Inc', 'Ltd', 'Co', 'Corp',
  // 月份 / 称谓
  'St', 'Jan', 'Feb', 'Mar', 'Apr',
  'Jun', 'Jul', 'Aug', 'Sep', 'Sept', 'Oct', 'Nov', 'Dec',
];

/**
 * 判断字符是否是大写字母（含 Latin-1 带重音大写字母）
 */
function isUpperCaseLetter(ch: string): boolean {
  return /[A-ZÀ-Þ]/.test(ch);
}

/**
 * 判断字符是否是字母（含 Latin-1 带重音字母）
 */
function isAsciiLetter(ch: string): boolean {
  return /[A-Za-zÀ-Þà-ÿ]/.test(ch);
}

/**
 * 将文本按句子边界切分
 *
 * 算法：字符级状态机扫描，对每个句末标点位置逐项判断是否构成真正的句子边界。
 *
 * 英文标点（. ! ?）边界判定，按优先级跳过：
 * 1. 数字 + 句点：跳过（小数 3.14、版本号 v1.0、章节号 3.2.、pH 7.4.）
 *    —— 代价是列表项 "1. The..." 也不再切，但论文正文段落中极少，且列表通常是独立 block
 * 2. 常见缩写（SENTENCE_ABBREVIATIONS）：跳过
 * 3. 单字母首字母缩写（A. B 模式，如 J. Smith、J. K. Rowling）：跳过
 *    —— 但若 prevPrev 也是句点（如 U.S.A. 最后一个 A.），视为连续缩写的末尾，应切
 * 4. 后续必须是 空白 + 大写字母（不再要求数字，避免 3.2. / v1.0. 误切）
 *
 * CJK 标点（。！？）边界判定：
 * - 直接切分，不要求后续大写字母
 *
 * @param text - 需要切分的文本
 * @returns 句子字符范围数组，每个元素包含起止位置和文本
 */
export function splitIntoSentences(text: string): SentenceCharRange[] {
  if (!text || text.trim().length === 0) return [];

  const sentences: SentenceCharRange[] = [];
  const len = text.length;
  let lastIndex = 0;

  /**
   * 预处理：识别"连续单字母缩写序列"（如 U.S.A.、U.K.、N.A.S.A.）
   *
   * 序列定义：至少 2 个连续的"大写字母 + 句点"（中间无空格）
   * 序列内部的所有句点（除最后一个）都跳过，避免在 U.S.A. 中间误切
   * 最后一个句点不跳过，交给主流程判定：
   *   - U.S.A. leads → leads 小写，不切
   *   - U.S.A. The  → The 大写，切（U.S.A. 是完整缩写词，后接新句子）
   */
  const sequenceDots = new Set<number>();
  const seqRe = /(?:[A-Z]\.){2,}/g;
  let seqMatch: RegExpExecArray | null;
  while ((seqMatch = seqRe.exec(text)) !== null) {
    const seqEnd = seqMatch.index + seqMatch[0].length;
    // 句点位置：index+1, index+3, ..., seqEnd-2
    // 最后一个句点（seqEnd-2）不加入，让主流程判定
    for (let p = seqMatch.index + 1; p < seqEnd - 2; p += 2) {
      sequenceDots.add(p);
    }
  }

  /**
   * 检查 pos 处的句点之前是否以缩写结尾（不区分大小写）
   * 缩写前必须是词边界（行首、空格、或非字母字符）
   */
  const isAbbreviated = (pos: number): boolean => {
    const before = text.slice(0, pos).toLowerCase();
    for (const abbr of SENTENCE_ABBREVIATIONS) {
      const abbrLower = abbr.toLowerCase();
      if (!before.endsWith(abbrLower)) continue;
      const charBeforeAbbr = before[before.length - abbrLower.length - 1];
      if (charBeforeAbbr === undefined || !isAsciiLetter(charBeforeAbbr)) {
        return true;
      }
    }
    return false;
  };

  /**
   * 检查 pos 处的句点是否构成单字母首字母缩写（J. Smith 模式）
   *
   * 条件：
   * - 前一字符是大写字母
   * - 前一字符的前一个字符不是句点（排除 U.S.A. 的最后一个 A.，交给主流程）
   * - 前一字符是"单字母"（其前一字符是空格、行首或非字母）
   * - 句点后（跳过空白）是大写字母
   * - 后续词长度 >= 3（如 Smith、Rowling），或后续是"大写字母+句点"（连续缩写 J. K.）
   *   —— 用长度区分 "J. Smith"（首字母）和 "shows X. It"（单字母句末）
   */
  const isSingleLetterInitial = (pos: number): boolean => {
    if (pos < 1) return false;
    const prevChar = text[pos - 1];
    if (!isUpperCaseLetter(prevChar)) return false;
    const prevPrev = pos >= 2 ? text[pos - 2] : '';
    if (prevPrev === '.') return false;
    const isSingle = !prevPrev || !isAsciiLetter(prevPrev);
    if (!isSingle) return false;
    let k = pos + 1;
    while (k < len && /\s/.test(text[k])) k++;
    if (k >= len || !isUpperCaseLetter(text[k])) return false;
    // 测量后续词长度（连续字母）
    let wordLen = 0;
    for (let j = k; j < len && isAsciiLetter(text[j]); j++) wordLen++;
    // 完整姓氏（>= 3 字母）或连续缩写（A. B. 模式）
    return wordLen >= 3 || (wordLen === 1 && k + 1 < len && text[k + 1] === '.');
  };

  /**
   * 检查 pos 处是否是图注项起始（单小写字母 + 逗号 + 空白 + 大写字母）
   *
   * 用于切分 figure caption 中 "a, Description. b, Description. c, Description." 这种
   * 模式 —— 主流程因后续是小写字母不切，但图注项实质上是独立"句子"。
   *
   * 形态约束严格（单字母 + 逗号 + 大写开头描述），避免误伤正文：
   *   ✓ biopsy. c, The large-scale...   (caption item)
   *   ✗ normal. a, b, c are variables... (列举，逗号后仍小写)
   */
  const isCaptionItemStart = (pos: number): boolean => {
    if (pos >= len) return false;
    if (!/[a-z]/.test(text[pos])) return false;
    if (pos + 1 >= len || text[pos + 1] !== ',') return false;
    let k = pos + 2;
    while (k < len && /\s/.test(text[k])) k++;
    return k < len && isUpperCaseLetter(text[k]);
  };

  /**
   * 提交一个句子 [lastIndex, end)，trim 后非空才入列
   */
  const commit = (end: number) => {
    if (end <= lastIndex) return;
    const sentenceText = text.slice(lastIndex, end).trim();
    if (sentenceText.length > 0) {
      sentences.push({
        start: lastIndex,
        end,
        text: sentenceText,
      });
    }
  };

  for (let i = 0; i < len; i++) {
    const ch = text[i];

    const isEnEnd = ch === '.' || ch === '!' || ch === '?';
    const isCjkEnd = ch === '。' || ch === '！' || ch === '？';
    if (!isEnEnd && !isCjkEnd) continue;

    // CJK 标点：直接切分（不需要后续大写字母确认）
    if (isCjkEnd) {
      commit(i + 1);
      lastIndex = i + 1;
      continue;
    }

    // 英文句点的额外跳过判定
    if (ch === '.') {
      // 连续单字母缩写序列内部（U.S.A. 的中间句点）
      if (sequenceDots.has(i)) continue;
      // 数字 + . + 数字：小数点（如 3.14、1.0、2.71），不切
      // 注意：数字 + . + 空白（如 7.4. The）仍是句子结束，应切
      if (
        i >= 1 &&
        /[0-9]/.test(text[i - 1]) &&
        i + 1 < len &&
        /[0-9]/.test(text[i + 1])
      ) {
        continue;
      }
      // 常见缩写
      if (isAbbreviated(i)) continue;
      // 单字母首字母缩写（J. Smith、J. K. Rowling）
      if (isSingleLetterInitial(i)) continue;
    }

    // 跳过紧随其后的引号 / 闭括号（如 ." '). ）
    let endIdx = i + 1;
    while (endIdx < len && /[)"'"']/.test(text[endIdx])) {
      endIdx++;
    }

    // 找到空白后的下一个非空白字符
    let nextIdx = endIdx;
    while (nextIdx < len && /\s/.test(text[nextIdx])) {
      nextIdx++;
    }

    // 文本末尾：提交最后一句
    if (nextIdx >= len) {
      commit(endIdx);
      lastIndex = endIdx;
      continue;
    }

    // 后续必须是大写字母（去掉数字，避免 3.2. / v1.0. / pH 7.4. 误切）
    // 图注项例外：". a, The..." / ". b, The..." 这类后接小写字母+逗号+大写字母
    // 是典型的 figure caption item 边界，应切（否则 caption b/c/d 会被错误合并成一句）
    if (!isUpperCaseLetter(text[nextIdx]) && !isCaptionItemStart(nextIdx)) continue;

    // 确认是句子边界
    commit(endIdx);
    lastIndex = endIdx;
  }

  // 处理最后一个句子
  if (lastIndex < len) {
    commit(len);
  }

  // 兜底：没有任何句子被切分出来时，整段作为单个句子
  if (sentences.length === 0) {
    const trimmed = text.trim();
    if (trimmed.length > 0) {
      sentences.push({
        start: 0,
        end: trimmed.length,
        text: trimmed,
      });
    }
  }

  return sentences;
}
