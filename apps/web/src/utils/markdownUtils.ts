/**
 * Markdown 处理工具模块
 *
 * 本模块提供 Markdown 文本的通用处理功能，包括：
 * - 按标题切分章节
 * - 章节内容的结构化处理
 * - Token 预算估算
 *
 * 为什么需要这些工具函数？
 * - AI 对话时需要根据 token 预算智能地注入论文内容
 * - 按章节切分可以保证内容的完整性，避免在段落中间截断
 * - Token 估算是前端必须做的，避免超出模型 context window 限制
 */

/**
 * 双语学术同义词映射表
 *
 * 用于扩展用户查询关键词，提高中英文学术论文的章节匹配准确率。
 * 每组同义词共享一个组 ID，查找时可以获取整组同义词。
 *
 * 结构：{ 术语: 组ID }
 */
const ACADEMIC_SYNONYMS = {
  // 方法相关
  'method': 1, 'methods': 1, '方法': 1, 'methodology': 1, 'approach': 1, 'approaches': 1,
  '算法': 1, 'algorithm': 1, 'algorithms': 1, '模型': 1, 'model': 1, 'models': 1,
  '框架': 1, 'framework': 1, 'frameworks': 1, '技术': 1, 'technique': 1, 'techniques': 1,

  // 结果相关
  'result': 2, 'results': 2, '结果': 2, '实验': 2, 'experiment': 2, 'experiments': 2,
  'performance': 2, '性能': 2, '效果': 2, 'evaluation': 2, '评估': 2, 'outcome': 2,
  'finding': 2, 'findings': 2, '发现': 2, 'metric': 2, 'metrics': 2, '指标': 2,

  // 引言相关
  'introduction': 3, '引言': 3, 'intro': 3, '背景': 3, 'background': 3,
  '导论': 3, '动机': 3, 'motivation': 3, 'overview': 3, '概述': 3,

  // 结论相关
  'conclusion': 4, 'conclusions': 4, '结论': 4, '总结': 4, 'summary': 4,
  '讨论': 4, 'discussion': 4, '未来工作': 4, 'future work': 4, 'future': 4,

  // 相关工作相关
  'related work': 5, '相关工作': 5, 'literature': 5, '文献': 5, '文献综述': 5,
  'literature review': 5, 'previous work': 5, '前人工作': 5, 'related': 5,

  // 摘要相关
  'abstract': 6, '摘要': 6, '概要': 6, '执行摘要': 6, 'executive summary': 6,

  // 数据相关
  'dataset': 7, 'datasets': 7, '数据集': 7, 'data': 7, '数据': 7,
  'database': 7, '数据库': 7, 'corpus': 7, '语料库': 7,

  // 分析相关
  'analysis': 8, 'analyses': 8, '分析': 8, '统计': 8, 'statistics': 8,
  'analytical': 8, 'statistical': 8, 'study': 8, '研究': 8,

  // 其他常见学术术语
  'implementation': 9, '实现': 9, 'system': 9, '系统': 9,
  'application': 10, '应用': 10, 'experiment setup': 11, '实验设置': 11,
  'limitations': 12, '限制': 12, 'limitation': 12,
};

/**
 * 反向索引：组ID → 同义词数组
 * 由 ACADEMIC_SYNONYMS 自动生成
 */
const SYNONYM_GROUPS: Record<string, string[]> = {};
for (const [term, groupId] of Object.entries(ACADEMIC_SYNONYMS)) {
  if (!SYNONYM_GROUPS[groupId]) {
    SYNONYM_GROUPS[groupId] = [];
  }
  SYNONYM_GROUPS[groupId].push(term);
}

/**
 * 将 Markdown 文本按标题切分为章节列表
 *
 * 该函数使用正则表达式识别 Markdown 标题（# 到 ####），并将文本切分为章节。
 * 切分结果保留标题和对应的内容块。
 *
 * 处理逻辑：
 * 1. 使用正则 `/^(#{1,4})\s+(.+)$/gm` 匹配 1-4 级 Markdown 标题
 * 2. 处理标题前的摘要/前言内容（如果有，归入"前言"章节）
 * 3. 处理无标题的情况（将整篇归入"全文"章节）
 * 4. split 结果格式为 [text, #'s, title, text, #'s, title, ...]
 * 5. 从索引 1 开始遍历，每 3 个元素一组（标题符号、标题文本、内容）
 *
 * @param {string} markdown - 原始 Markdown 文本
 * @returns {Array<{title: string, content: string, fullText: string}>} 章节数组
 *   每个元素包含：
 *   - title: 章节标题
 *   - content: 章节内容（不包含标题行本身）
 *   - fullText: 包含标题行在内的完整文本
 *
 * @example
 * const markdown = "# Title\nContent\n## Section\nMore content";
 * const sections = splitMarkdownBySections(markdown);
 * // 返回: [{title: "Title", content: "Content", fullText: "# Title\nContent"}, ...]
 */
export function splitMarkdownBySections(markdown: string): Array<{ title: string; content: string; fullText: string }> {
  if (!markdown || !markdown.trim()) {
    return [];
  }

  // ========== 编译正则表达式 ==========
  // 匹配 Markdown 标题语法：1-4 个井号开头，后跟空格和标题文本
  // gm 标志：g 全局匹配，m 多行模式（^ 能匹配每行开头）
  const sectionPattern = /^(#{1,4})\s+(.+)$/gm;

  // ========== 使用正则切分 Markdown 文本 ==========
  // split 结果格式：[文本, #符号, 标题, 文本, #符号, 标题, ...]
  // 例如："Abstract\n# Intro\nContent" → ["Abstract\n", "#", "Intro", "\nContent"]
  const sections = markdown.split(sectionPattern);

  // ========== 初始化章节块列表 ==========
  const sectionBlocks = [];

  // ========== 处理标题前的摘要/前言内容 ==========
  // 如果第一段不是标题（即不匹配 # 开头），则认为是前言/摘要部分
  const firstSection = sections[0];
  if (firstSection && !sectionPattern.test(firstSection.trim())) {
    const preface = firstSection.trim();
    if (preface) {
      sectionBlocks.push({
        title: '前言',
        content: preface,
        fullText: preface,
      });
    }
  }

  // ========== 解析正则切分结果 ==========
  // sections 格式为 [text, #'s, title, text, #'s, title, ...]
  // 从索引 1 开始遍历，每 3 个元素为一组（标题符号、标题文本、内容）
  let i = 1;
  while (i < sections.length) {
    if (i + 2 < sections.length) {
      const headingSymbol = sections[i];       // # 符号（如 "##"）
      const title = sections[i + 1].trim();     // 章节标题
      const content = sections[i + 2].trim();   // 章节内容

      if (content) {
        // 构建完整文本（包含标题行）
        const fullText = `${headingSymbol} ${title}\n\n${content}`;
        sectionBlocks.push({
          title,
          content: content || '',
          fullText,
        });
      }
      i += 3;
    } else {
      break;
    }
  }

  // ========== 处理无标题的情况 ==========
  // 如果没有找到任何标题且原文非空，将整篇作为一个"全文"章节
  if (sectionBlocks.length === 0 && markdown.trim()) {
    sectionBlocks.push({
      title: '全文',
      content: markdown.trim(),
      fullText: markdown.trim(),
    });
  }

  return sectionBlocks;
}

/**
 * 估算文本的 Token 数量
 *
 * 为什么需要这个函数？
 * - 浏览器中没有 tiktoken（Python 库），无法精确计算
 * - Token 预算控制不需要精确值，保守估算即可
 * - 避免超出模型的 context window 限制
 *
 * 估算策略（加权公式）：
 * - 中文字符：约 1.5 字符 = 1 token
 * - 英文/其他字符：约 3.5 字符 = 1 token
 * - 分别计数后加权求和，准确处理混合学术文本
 *
 * @param {string} text - 需要估算的文本内容
 * @returns {number} 估算的 token 数量（整数）
 */
export function estimateTokens(text: string): number {
  if (!text) return 0;

  // 统计中文字符数量（Unicode 范围：\u4e00-\u9fff）
  const chineseChars = (text.match(/[\u4e00-\u9fff]/g) || []).length;
  const otherChars = text.length - chineseChars;

  // 加权计算：中文每字符约 1.5 token，英文/其他每 3.5 字符约 1 token
  return Math.ceil(chineseChars * 1.5 + otherChars / 3.5);
}

/**
 * 根据用户消息的相关性选择章节内容
 *
 * 该函数实现基于相关性的章节选择策略：
 * 1. 将 Markdown 按章节切分
 * 2. 根据用户消息的关键词匹配章节标题和内容，计算相关性分数
 * 3. 如果提供了语义数据，使用 LLM 生成的 topics 和 mainFinding 进行语义增强匹配
 * 4. 优先注入高相关性章节
 * 5. 始终包含摘要/前言等优先章节
 * 6. 按相关性排序后注入，直到预算用尽
 *
 * @param {string} markdown - 论文的完整 Markdown 内容
 * @param {number} remainingBudget - 剩余可用的 token 预算
 * @param {string} userMessage - 用户的消息内容，用于计算相关性
 * @param {Array<Object>|null} sectionSemantics - 章节语义元数据数组（可选），由后端 LLM 预生成
 *   每个元素格式：{ title: string, topics: string[], contentType: string, mainFinding: string }
 *   - title: 章节标题（用于匹配章节）
 *   - topics: 双语关键术语数组（权重 2x）
 *   - contentType: 内容类型枚举值（methodology|results|background|discussion|conclusion|other）
 *   - mainFinding: 核心发现摘要（权重 1.5x）
 *   传入 null 或不传时，行为与纯关键词匹配完全一致（向后兼容）
 * @returns {{content: string, injectedSections: Array<string>, estimatedTokens: number, omittedSections: Array<Object>}}
 *   - content: 注入的内容（可能包含多个章节）
 *   - injectedSections: 注入的章节标题列表
 *   - estimatedTokens: 估算的 token 数量
 *   - omittedSections: 未注入的章节及其分数
 */
export function selectSectionsByRelevance(markdown: string, remainingBudget: number, userMessage: string, sectionSemantics: Array<{ title: string; topics?: string[]; contentType?: string; mainFinding?: string }> | null = null): { content: string; injectedSections: string[]; estimatedTokens: number; omittedSections: Array<{ title: string; score: number }> } {
  const sections = splitMarkdownBySections(markdown);

  if (sections.length === 0) {
    return { content: '', injectedSections: [], estimatedTokens: 0, omittedSections: [] };
  }

  const MIN_BUDGET = 500;
  // 优先章节标题列表：这些章节总是应该优先注入
  const PRIORITY_TITLES = [
    'abstract', '摘要', '前言',
    'introduction', '引言', '导论',
    'conclusion', '结论', '总结',
  ];

  // 定义停用词集合（中英文常用停用词）
  const stopWords = new Set([
    '的', '了', '是', '在', '有', '和', '与', '也', '都', '这', '那', '就', '不', '我', '他', '她',
    'the', 'a', 'an', 'is', 'are', 'was', 'were', 'be', 'been', 'being', 'what', 'how', 'why',
    'do', 'does', 'did', 'can', 'could', 'would', 'should', 'will', 'shall', 'may', 'might',
    'this', 'that', 'it', 'its', 'me', 'my', 'we', 'our', 'you', 'your', 'he', 'she', 'they', 'their',
    'of', 'in', 'to', 'for', 'with', 'on', 'at', 'by', 'from', 'about', 'into', 'through',
    'during', 'before', 'after', 'above', 'below', 'between', 'out', 'off', 'over', 'under',
    'again', 'further', 'then', 'once', 'here', 'there', 'when', 'where', 'why', 'how',
    'all', 'both', 'each', 'few', 'more', 'most', 'other', 'some', 'such', 'no', 'nor',
    'not', 'only', 'own', 'same', 'so', 'than', 'too', 'very', 'just', 'because', 'as',
    'until', 'while', 'if', 'or', 'but', 'and'
  ]);

  // 从用户消息中提取关键词
  let keywords = (userMessage || '')
    .toLowerCase()
    .split(/[\s,，。.？！?！;；：:、]+/)
    .filter(w => w.length > 1 && !stopWords.has(w));

  /**
   * 从 CJK 关键词中提取 bigram 子串
   * 例如： "实验结果" → ["实验", "验结", "结果"]
   *
   * @param {string} keyword - 原始关键词
   * @returns {string[]} bigram 数组
   */
  function extractCJKBigrams(keyword: string): string[] {
    const bigrams = [];
    // 匹配连续的中文字符（CJK Unified Ideographs）
    const cjkChars = keyword.match(/[\u4e00-\u9fff]+/g);
    if (cjkChars) {
      for (const segment of cjkChars) {
        // 只对长度 > 2 的中文片段提取 bigram
        if (segment.length > 2) {
          for (let i = 0; i <= segment.length - 2; i++) {
            const bigram = segment.substring(i, i + 2);
            if (bigram.length === 2) {
              bigrams.push(bigram);
            }
          }
        }
      }
    }
    return bigrams;
  }

  // ========== 步骤1：提取 CJK bigram 子串 ==========
  // 对包含中文字符且长度 > 2 的关键词，提取其 bigram 子串作为补充关键词
  const bigramKeywords = [];
  for (const kw of keywords) {
    const bigrams = extractCJKBigrams(kw);
    bigramKeywords.push(...bigrams);
  }
  keywords.push(...bigramKeywords);

  // ========== 步骤2：使用学术同义词映射扩展关键词 ==========
  // 检查每个关键词是否在同义词映射表中，如果在则将同组所有术语加入关键词列表
  const expandedKeywords = [];
  for (const kw of keywords) {
    expandedKeywords.push(kw); // 保留原始关键词

    const groupId = (ACADEMIC_SYNONYMS as unknown as Record<string, string>)[kw];
    if (groupId && SYNONYM_GROUPS[groupId]) {
      // 将同组的所有同义词都加入关键词列表
      for (const synonym of SYNONYM_GROUPS[groupId]) {
        if (synonym !== kw && !expandedKeywords.includes(synonym)) {
          expandedKeywords.push(synonym);
        }
      }
    }
  }
  keywords = expandedKeywords;

  // 为每个章节计算相关性分数
  const scored = sections.map(section => {
    const titleLower = section.title.toLowerCase();

    // 采样内容：同时采样头部和尾部，提高相关性判断准确性
    const contentLen = section.content.length;
    const headSample = section.content.slice(0, 200).toLowerCase();
    const tailSample = contentLen > 200
      ? section.content.slice(Math.max(0, contentLen - 200)).toLowerCase()
      : '';
    const contentSample = headSample + ' ' + tailSample;

    let score = 0;

    for (const kw of keywords) {
      // 标题匹配权重为 3x
      if (titleLower.includes(kw)) score += 3;
      // 内容匹配权重为 1x
      if (contentSample.includes(kw)) score += 1;
    }

    // ========== 语义增强匹配（如果提供了语义元数据）==========
    // 如果后端已生成章节语义数据（topics + mainFinding），使用这些富语义数据进行匹配
    // 这解决了纯关键词匹配无法理解语义的问题（如"创新点在哪里"这类抽象问题）
    if (sectionSemantics && Array.isArray(sectionSemantics)) {
      // 根据章节标题查找对应的语义元数据
      const semantics = sectionSemantics.find(s => s.title === section.title);
      if (semantics) {
        // ===== 匹配 LLM 生成的 topics（权重 2x）=====
        // topics 是 LLM 提取的双语关键术语，比简单关键词匹配更能反映章节语义
        // 权重 2x：介于标题匹配（3x）和内容匹配（1x）之间
        if (semantics.topics && Array.isArray(semantics.topics)) {
          // 将所有 topics 转为小写，便于大小写不敏感匹配
          const semTopicsLower = semantics.topics.map(t => String(t).toLowerCase());

          for (const kw of keywords) {
            for (const topic of semTopicsLower) {
              // 双向子串匹配：
              // 1. topic 包含关键词（如 "transformer" 包含 "transform"）
              // 2. 关键词包含 topic（如 "注意力" 包含 "注意力机制" 的一部分）
              // 这种双向匹配能捕捉部分匹配和层级关系
              if (topic.includes(kw) || kw.includes(topic)) {
                score += 2; // 语义 topics 匹配权重为 2x
              }
            }
          }
        }

        // ===== 匹配 LLM 提炼的 mainFinding（权重 1.5x）=====
        // mainFinding 是 LLM 用 1-2 句话概括的章节核心发现
        // 权重 1.5x：略高于内容匹配，因为它是精炼的语义而非原始文本
        if (semantics.mainFinding && typeof semantics.mainFinding === 'string') {
          const findingLower = semantics.mainFinding.toLowerCase();

          for (const kw of keywords) {
            // 子串匹配：用户关键词出现在核心发现描述中
            if (findingLower.includes(kw)) {
              score += 1.5; // mainFinding 匹配权重为 1.5x
            }
          }
        }
      }
    }

    // 优先章节（摘要/前言）给予额外加分
    if (PRIORITY_TITLES.some(t => titleLower.includes(t))) score += 10;

    return { ...section, score };
  });

  // 按分数降序排序
  scored.sort((a, b) => b.score - a.score);

  // 按预算注入章节（非贪心策略：允许跳过大章节后继续检查小章节）
  let selectedContent = '';
  const injectedSections = [];
  let usedTokens = 0;
  let partiallyInjected = false; // 标记是否已经做过部分注入（只做一次）

  for (const section of scored) {
    const sectionTokens = estimateTokens(section.fullText);
    const remaining = remainingBudget - usedTokens;

    // 预算已用尽，停止
    if (remaining <= 0) break;

    // 当前章节装不下
    if (usedTokens + sectionTokens > remainingBudget) {
      // 只对第一个装不下的章节做部分注入（最高相关但装不下的那个）
      if (!partiallyInjected && remaining > MIN_BUDGET) {
        const ratio = remaining / sectionTokens;
        const partialContent = section.content.slice(0, Math.floor(section.content.length * ratio));
        selectedContent += `\n\n## ${section.title}\n${partialContent}...\n\n[注：由于 token 预算限制，本章节仅部分注入]`;
        injectedSections.push(section.title + '（部分）');
        usedTokens += remaining;
        partiallyInjected = true;
      }
      // ← 不要 break，继续检查后续可能更小的章节
      continue;
    }

    // 当前章节可以完全注入
    selectedContent += (selectedContent ? '\n\n' : '') + section.fullText;
    injectedSections.push(section.title);
    usedTokens += sectionTokens;
  }

  // 计算被省略的章节（未注入的章节，按相关性排序）
  const injectedSet = new Set(injectedSections.map(s => s.replace('（部分）', '')));
  const omittedSections = scored
    .filter(s => !injectedSet.has(s.title))
    .map(s => ({ title: s.title, score: s.score }))
    .sort((a, b) => b.score - a.score);

  return {
    content: selectedContent,
    injectedSections,
    estimatedTokens: usedTokens,
    omittedSections, // 新增：被省略章节的信息，用于降级提示
  };
}
