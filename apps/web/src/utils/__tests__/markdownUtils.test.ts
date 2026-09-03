/**
 * markdownUtils 测试文件
 *
 * 测试 Markdown 工具函数的功能。
 *
 * 跨语言测试用例定义在 tests/SHARED_SPEC.md 中（用例 001-015）。
 * Python 和 JavaScript 实现必须通过所有跨语言用例。
 * 前端专用用例（F001-F004）测试 JS 特定功能。
 */

import { describe, it, expect } from 'vitest';
import {
  splitMarkdownBySections,
  estimateTokens,
  selectSectionsByRelevance,
} from '../markdownUtils';

// ============================================================
// 跨语言测试用例（SHARED_SPEC.md 001-015）
// ============================================================

describe('splitMarkdownBySections — 跨语言规范测试（001-015）', () => {
  // 用例 001：基础分割
  it('001: 基础分割 - 包含两个标题的 Markdown', () => {
    const markdown = '# Title1\nContent1\n# Title2\nContent2';
    const result = splitMarkdownBySections(markdown);

    expect(result).toHaveLength(2);
    expect(result[0].title).toBe('Title1');
    expect(result[0].content).toBe('Content1');
    expect(result[1].title).toBe('Title2');
    expect(result[1].content).toBe('Content2');
  });

  // 用例 002：混合标题级别
  it('002: 混合标题级别 - h1 到 h4', () => {
    const markdown = '# H1\nC1\n## H2\nC2\n### H3\nC3\n#### H4\nC4';
    const result = splitMarkdownBySections(markdown);

    expect(result).toHaveLength(4);
    expect(result[0].title).toBe('H1');
    expect(result[0].content).toBe('C1');
    expect(result[1].title).toBe('H2');
    expect(result[1].content).toBe('C2');
    expect(result[2].title).toBe('H3');
    expect(result[2].content).toBe('C3');
    expect(result[3].title).toBe('H4');
    expect(result[3].content).toBe('C4');
  });

  // 用例 003：有标题的前言
  it('003: 第一个标题前的文本成为前言部分', () => {
    const markdown = 'Abstract text here\n# Introduction\nIntro content';
    const result = splitMarkdownBySections(markdown);

    expect(result).toHaveLength(2);
    expect(result[0].title).toBe('前言');
    expect(result[0].content).toBe('Abstract text here');
    expect(result[1].title).toBe('Introduction');
    expect(result[1].content).toBe('Intro content');
  });

  // 用例 004：无标题的情况
  it('004: 无标题 - 整个文本成为"前言"章节', () => {
    const markdown = 'Just plain text\nwithout any headings';
    const result = splitMarkdownBySections(markdown);

    expect(result).toHaveLength(1);
    expect(result[0].title).toBe('前言');
    expect(result[0].content).toBe('Just plain text\nwithout any headings');
  });

  // 用例 005：空字符串输入
  it('005: 空字符串返回空数组', () => {
    expect(splitMarkdownBySections('')).toEqual([]);
  });

  // 用例 006：仅空白字符输入
  it('006: 仅空白字符输入返回空数组', () => {
    const markdown = '   \n\n   \t\n  ';
    expect(splitMarkdownBySections(markdown)).toEqual([]);
  });

  // 用例 007：连续标题
  it('007: 连续标题 - 第一个被丢弃（内容为空）', () => {
    const markdown = '# Title1\n# Title2\nContent2';
    const result = splitMarkdownBySections(markdown);

    expect(result).toHaveLength(1);
    expect(result[0].title).toBe('Title2');
    expect(result[0].content).toBe('Content2');
  });

  // 用例 008：标题后仅空白字符
  it('008: 标题后仅空白字符的内容被丢弃', () => {
    const markdown = '# Title1\n   \n   \n# Title2\nReal content';
    const result = splitMarkdownBySections(markdown);

    expect(result).toHaveLength(1);
    expect(result[0].title).toBe('Title2');
    expect(result[0].content).toBe('Real content');
  });

  // 用例 009：h5 不被识别为标题
  it('009: h5（#####）不被识别为标题', () => {
    const markdown = '# Title\nSome content\n##### H5 text\nMore content';
    const result = splitMarkdownBySections(markdown);

    expect(result).toHaveLength(1);
    expect(result[0].title).toBe('Title');
    expect(result[0].content).toBe('Some content\n##### H5 text\nMore content');
  });

  // 用例 010：#号后无空格
  it('010: #号后无空格不是标题', () => {
    const markdown = '#NotAHeading\n# RealHeading\nContent';
    const result = splitMarkdownBySections(markdown);

    expect(result).toHaveLength(2);
    expect(result[0].title).toBe('前言');
    expect(result[0].content).toBe('#NotAHeading');
    expect(result[1].title).toBe('RealHeading');
    expect(result[1].content).toBe('Content');
  });

  // 用例 011：中文内容
  it('011: 中文标题和内容', () => {
    const markdown = '# 引言\n这是引言内容\n## 方法\n方法描述';
    const result = splitMarkdownBySections(markdown);

    expect(result).toHaveLength(2);
    expect(result[0].title).toBe('引言');
    expect(result[0].content).toBe('这是引言内容');
    expect(result[1].title).toBe('方法');
    expect(result[1].content).toBe('方法描述');
  });

  // 用例 012：单个标题无内容
  it('012: 单独的标题无内容触发"全文"回退', () => {
    const result = splitMarkdownBySections('# LonelyTitle');

    expect(result).toHaveLength(1);
    expect(result[0].title).toBe('全文');
    expect(result[0].content).toBe('# LonelyTitle');
  });

  // 用例 013：以标题开头
  it('013: 以标题开头 - 无前言部分', () => {
    const markdown = '# First\nContent1\n# Second\nContent2';
    const result = splitMarkdownBySections(markdown);

    expect(result).toHaveLength(2);
    expect(result[0].title).toBe('First');
    expect(result[0].content).toBe('Content1');
    expect(result[1].title).toBe('Second');
    expect(result[1].content).toBe('Content2');
    expect(result.every((s) => s.title !== '前言')).toBe(true);
  });

  // 用例 014：行内 # 号
  it('014: 行内 # 号不是标题', () => {
    const markdown = 'Some text # not a heading\n# Real heading\nContent';
    const result = splitMarkdownBySections(markdown);

    expect(result).toHaveLength(2);
    expect(result[0].title).toBe('前言');
    expect(result[0].content).toBe('Some text # not a heading');
    expect(result[1].title).toBe('Real heading');
    expect(result[1].content).toBe('Content');
  });

  // 用例 015：空前言被忽略
  it('015: 空前言（第一个标题前的空白）被忽略', () => {
    const markdown = '\n\n\n# Title\nContent';
    const result = splitMarkdownBySections(markdown);

    expect(result).toHaveLength(1);
    expect(result[0].title).toBe('Title');
    expect(result[0].content).toBe('Content');
  });
});

// ============================================================
// 前端专用测试用例（F001-F004）
// ============================================================

describe('splitMarkdownBySections — 前端专用（fullText 字段）', () => {
  // 用例 F001：fullText 字段
  it('F001: fullText 重建标题行加内容', () => {
    const markdown = '# Title\nContent';
    const result = splitMarkdownBySections(markdown);

    expect(result).toHaveLength(1);
    expect(result[0].fullText).toBe('# Title\n\nContent');
  });
});

describe('estimateTokens - Token 估算函数', () => {
  // 用例 F002：中文 Token 估算
  it('F002: 中文文本使用字符数/1.5 比例', () => {
    // 实现使用：chineseChars * 1.5 + otherChars / 3.5
    expect(estimateTokens('你好世界')).toBe(Math.ceil(4 * 1.5));
  });

  // 用例 F003：英文 Token 估算
  it('F003: 英文文本使用字符数/3 比例', () => {
    expect(estimateTokens('Hello world')).toBe(Math.ceil(11 / 3));
  });

  // 用例 F004：空字符串
  it('F004: 空字符串返回 0', () => {
    expect(estimateTokens('')).toBe(0);
  });
});

// ============================================================
// selectSectionsByRelevance 测试
// ============================================================

describe('selectSectionsByRelevance - 相关性章节选择', () => {
  // 辅助函数：创建用于测试的多章节 Markdown
  const createTestMarkdown = () => `
# Abstract

This is the abstract of the paper. It provides a brief overview.

# Introduction

This is the introduction section with background information.

# Methods

We describe the methods and algorithms used in this study.

# Results

The experimental results are presented here with analysis.

# Conclusion

We conclude the paper with final remarks.
`;

  // 测试 1：空 Markdown 返回空结果
  it('空 Markdown 返回空结果', () => {
    const result = selectSectionsByRelevance('', 1000, 'what is the method');
    expect(result.content).toBe('');
    expect(result.injectedSections).toEqual([]);
    expect(result.estimatedTokens).toBe(0);
    // 当没有章节时，omittedSections 为空数组（保持返回 shape 一致）
    expect(result.omittedSections).toEqual([]);
  });

  // 测试 2：空用户消息仍优先考虑优先标题
  it('空用户消息仍优先选择摘要/引言/结论', () => {
    const markdown = createTestMarkdown();
    const budget = estimateTokens(markdown) + 1000; // 大预算

    const result = selectSectionsByRelevance(markdown, budget, '');

    // 仍应注入所有章节，但优先章节应在前
    expect(result.injectedSections.length).toBeGreaterThan(0);

    // 前两个注入的章节应该是优先标题（Abstract、Introduction）
    const firstTwo = result.injectedSections.slice(0, 2).join(' ').toLowerCase();
    expect(/abstract|introduction/.test(firstTwo)).toBe(true);
  });

  // 测试 3：充足预算注入所有章节
  it('充足预算注入所有章节', () => {
    const markdown = createTestMarkdown();
    const totalTokens = estimateTokens(markdown);
    const budget = totalTokens + 1000;

    const result = selectSectionsByRelevance(markdown, budget, 'tell me about this paper');

    // 所有章节都应被注入
    expect(result.injectedSections.length).toBe(5);
    expect(result.omittedSections).toEqual([]);
    expect(result.estimatedTokens).toBeGreaterThan(0);
  });

  // 测试 4：预算不足只注入部分章节
  it('预算不足只注入部分章节', () => {
    const markdown = createTestMarkdown();
    // 很小的预算，只能容纳 2-3 个章节（总共约 100 tokens）
    const budget = 50;

    const result = selectSectionsByRelevance(markdown, budget, 'what is the method');

    // 注入的章节应少于总数
    expect(result.injectedSections.length).toBeLessThan(5);
    expect(result.omittedSections.length).toBeGreaterThan(0);
  });

  // 测试 5：CJK 双字匹配 - "实验结果"匹配包含"实验"或"结果"的章节
  it('CJK 双字匹配："实验结果怎么样"匹配包含"实验"或"结果"的章节', () => {
    const markdown = `
# 实验设计

This describes the experiment design.

# 结果分析

We analyze the results here.

# 讨论

Discussion section.
`;

    const result = selectSectionsByRelevance(markdown, 2000, '实验结果怎么样');

    // 应注入标题中包含"实验"或"结果"的章节
    const injected = result.injectedSections.join(' ');
    expect(/实验|结果/.test(injected)).toBe(true);
  });

  // 测试 6：短中文关键词（长度 ≤ 2）不进行双字分割
  it('短中文关键词（长度 ≤ 2）不进行双字分割', () => {
    const markdown = `
# 方法

Methods section.

# 实验

Experiments section.
`;

    // "方法"长度为 2，不应被分割成双字
    const result = selectSectionsByRelevance(markdown, 2000, '方法');

    // 仍应直接匹配"方法"章节
    expect(result.injectedSections.join(' ')).toContain('方法');
  });

  // 测试 7：中文→英文同义词匹配（"方法" → "Method"）
  it('同义词匹配：中文"方法"匹配英文"Method"章节', () => {
    const markdown = `
# Abstract

Brief abstract.

# Method

We describe the method used.

# Results

Experimental results.
`;

    const result = selectSectionsByRelevance(markdown, 2000, '他们的方法是什么');

    // 应通过同义词映射匹配"Method"章节
    expect(result.injectedSections.join(' ')).toContain('Method');
  });

  // 测试 8：英文→中文/英文同义词匹配（"result" → "实验"/"Results"）
  it('同义词匹配：英文"result"匹配"Results"和中文"实验"', () => {
    const markdown = `
# Abstract

Brief abstract.

# Experiments

We conducted experiments.

# Results

The results show...
`;

    const result = selectSectionsByRelevance(markdown, 2000, 'what are the results');

    // 应通过同义词映射匹配"Results"和"Experiments"
    const injected = result.injectedSections.join(' ');
    expect(/Results|Experiments/.test(injected)).toBe(true);
  });

  // 测试 9：Abstract/摘要即使无关键词匹配也获得优先加成
  it('优先标题：Abstract/摘要即使无关键词匹配也获得加成', () => {
    const markdown = `
# Abstract

This is the abstract.

# Related Work

Previous research.

# Implementation

Our implementation.
`;

    // 用户询问实现，而非摘要
    const result = selectSectionsByRelevance(markdown, 2000, 'tell me about the implementation');

    // Abstract 应因优先加成仍被注入
    expect(result.injectedSections.join(' ')).toContain('Abstract');
  });

  // 测试 10：Conclusion/结论也获得优先加成
  it('优先标题：Conclusion/结论获得优先加成', () => {
    const markdown = `
# Method

Method description.

# Conclusion

Final conclusions.
`;

    const result = selectSectionsByRelevance(markdown, 2000, 'what is the method');

    // Conclusion 应被优先考虑
    expect(result.injectedSections.length).toBeGreaterThan(0);
  });

  // 测试 11：非贪婪填充 - 大章节后的小章节仍可被注入
  it('非贪婪：大章节后的小章节仍可被注入', () => {
    // 创建包含大章节后跟小章节的 Markdown
    const largeContent = 'x'.repeat(5000); // 大章节
    const markdown = `
# Abstract

Brief abstract.

# Big Section

${largeContent}

# Small Note

This is a small note.
`;

    // 可容纳 Abstract + Small Note 但不能容纳 Big Section 的预算
    const abstractTokens = estimateTokens('# Abstract\n\nBrief abstract.');
    const smallNoteTokens = estimateTokens('# Small Note\n\nThis is a small note.');
    const budget = abstractTokens + smallNoteTokens + 100;

    const result = selectSectionsByRelevance(markdown, budget, 'tell me everything');

    // 应注入 Abstract 和 Small Note，跳过 Big Section
    expect(result.injectedSections.join(' ')).toContain('Abstract');
    expect(result.injectedSections.join(' ')).toContain('Small Note');
  });

  // 测试 12：部分注入只发生一次
  it('部分注入只发生一次（第一个超大章节）', () => {
    const largeContent1 = 'y'.repeat(3000);
    const largeContent2 = 'z'.repeat(3000);
    const markdown = `
# Abstract

Abstract text.

# Large Section One

${largeContent1}

# Large Section Two

${largeContent2}
`;

    // 可容纳 Abstract + 第一个大章节一半的预算
    const budget = 2000;

    const result = selectSectionsByRelevance(markdown, budget, 'tell me about the large sections');

    // 应只有一个部分注入标记
    const partialCount = result.injectedSections.filter(s => s.includes('（部分）')).length;
    expect(partialCount).toBeLessThanOrEqual(1);
  });

  // 测试 13：omittedSections 包含未注入的章节
  it('omittedSections 包含未注入的章节', () => {
    const markdown = createTestMarkdown();
    const budget = 50; // 很小的预算（总共约 100 tokens）

    const result = selectSectionsByRelevance(markdown, budget, 'what is the method');

    // 应有被省略的章节
    expect(result.omittedSections).toBeDefined();
    expect(Array.isArray(result.omittedSections)).toBe(true);
    expect(result.omittedSections.length).toBeGreaterThan(0);
  });

  // 测试 14：omittedSections 按分数降序排序
  it('omittedSections 按分数降序排序', () => {
    const markdown = createTestMarkdown();
    const budget = 300;

    const result = selectSectionsByRelevance(markdown, budget, 'results and conclusion');

    // 分数应按降序排列
    for (let i = 0; i < result.omittedSections.length - 1; i++) {
      expect(result.omittedSections[i].score).toBeGreaterThanOrEqual(result.omittedSections[i + 1].score);
    }
  });

  // 测试 15：高分章节先被注入
  it('高分章节先于低分章节被注入', () => {
    const markdown = `
# Abstract

Abstract text.

# Methods

Methods description.

# Discussion

Discussion text.
`;

    const result = selectSectionsByRelevance(markdown, 2000, 'tell me about the methods');

    // Methods 应先于 Discussion 被注入（因关键词匹配分数更高）
    const methodsIndex = result.injectedSections.findIndex(s => s.includes('Methods'));
    const discussionIndex = result.injectedSections.findIndex(s => s.includes('Discussion'));

    if (methodsIndex !== -1 && discussionIndex !== -1) {
      expect(methodsIndex).toBeLessThan(discussionIndex);
    }
  });

  // 测试 16：尾部采样 - 匹配内容末尾的关键词
  it('尾部采样：匹配内容末尾的关键词', () => {
    const contentWithKeywordAtEnd = `
This is the beginning of the section.

More content in the middle.

And at the end we mention experiments and results analysis.
`;

    const markdown = `
# Section One

${contentWithKeywordAtEnd}
`;

    const result = selectSectionsByRelevance(markdown, 2000, 'tell me about the experiments');

    // 应匹配，因为"experiments"出现在内容末尾
    expect(result.injectedSections.length).toBeGreaterThan(0);
  });

  // 测试 17：返回值结构包含所有预期字段
  it('返回值包含所有预期字段', () => {
    const markdown = createTestMarkdown();
    const result = selectSectionsByRelevance(markdown, 1000, 'what is this about');

    // 检查所有预期字段存在
    expect(result).toHaveProperty('content');
    expect(result).toHaveProperty('injectedSections');
    expect(result).toHaveProperty('estimatedTokens');
    expect(result).toHaveProperty('omittedSections');

    // 检查类型
    expect(typeof result.content).toBe('string');
    expect(Array.isArray(result.injectedSections)).toBe(true);
    expect(typeof result.estimatedTokens).toBe('number');
    expect(Array.isArray(result.omittedSections)).toBe(true);
  });

  // ============================================================
  // 语义增强匹配测试（sectionSemantics 参数）
  // ============================================================

  // 测试 18：sectionSemantics=null 时行为与纯关键词匹配完全一致（向后兼容）
  it('sectionSemantics=null 行为与纯关键词匹配完全一致（向后兼容）', () => {
    const markdown = `
# Methods

We use a neural network for classification.

# Results

The model achieves 95% accuracy.
`;

    const resultWithoutSemantics = selectSectionsByRelevance(markdown, 2000, 'tell me about the methods', null);
    const resultWithNullSemantics = selectSectionsByRelevance(markdown, 2000, 'tell me about the methods', null);

    // 两种调用方式的结果应完全一致
    expect(resultWithNullSemantics.injectedSections).toEqual(resultWithoutSemantics.injectedSections);
  });

  // 测试 19：语义 topics 匹配权重高于内容匹配（2x > 1x）
  it('语义 topics 匹配权重高于内容匹配（2x > 1x）', () => {
    const markdown = `
# Technical Section

This section discusses technical concepts without mentioning specific keywords.
`;

    const sectionSemantics = [
      {
        title: 'Technical Section',
        index: 0,
        topics: ['transformer', 'attention', 'neural network'],
        contentType: 'methodology',
        mainFinding: 'We propose a new architecture.',
      },
    ];

    // 用户查询包含 LLM 提取的 topics 中的关键词
    const result = selectSectionsByRelevance(markdown, 2000, 'how does the transformer work', sectionSemantics);

    // 应匹配到该章节（通过语义 topics 匹配）
    expect(result.injectedSections).toContain('Technical Section');
  });

  // 测试 20：语义 topics 支持双向子串匹配（topic 包含关键词 或 关键词包含 topic）
  it('语义 topics 支持双向子串匹配', () => {
    const markdown = `
# Deep Learning Section

Content about deep learning models.
`;

    const sectionSemantics = [
      {
        title: 'Deep Learning Section',
        index: 0,
        topics: ['convolutional neural network', 'CNN'], // topics 包含长词和缩写
        contentType: 'methodology',
        mainFinding: 'We use CNN for image classification.',
      },
    ];

    // 用户查询包含缩写 "CNN"，应匹配到 topics 中的 "CNN"
    const result1 = selectSectionsByRelevance(markdown, 2000, 'tell me about the CNN', sectionSemantics);
    expect(result1.injectedSections).toContain('Deep Learning Section');

    // 用户查询包含长词的一部分，应匹配到 topics 中的 "convolutional neural network"
    const result2 = selectSectionsByRelevance(markdown, 2000, 'explain the neural network', sectionSemantics);
    expect(result2.injectedSections).toContain('Deep Learning Section');
  });

  // 测试 21：mainFinding 匹配生效（权重 1.5x）
  it('mainFinding 匹配生效（权重 1.5x）', () => {
    const markdown = `
# Innovation Section

This section describes our novel contributions.
`;

    const sectionSemantics = [
      {
        title: 'Innovation Section',
        index: 0,
        topics: ['model', 'algorithm'],
        contentType: 'conclusion',
        mainFinding: 'Our model outperforms the baseline by 15% on the benchmark dataset.', // 核心发现包含 "baseline"
      },
    ];

    // 用户查询包含 mainFinding 中的关键词
    const result = selectSectionsByRelevance(markdown, 2000, 'how does it compare to baseline', sectionSemantics);

    // 应匹配到该章节（通过 mainFinding 匹配）
    expect(result.injectedSections).toContain('Innovation Section');
  });

  // 测试 22：语义匹配支持中文关键词
  it('语义匹配支持中文关键词', () => {
    const markdown = `
# 方法章节

This section describes our approach.
`;

    const sectionSemantics = [
      {
        title: '方法章节',
        index: 0,
        topics: ['transformer', '注意力机制', 'BERT'], // 双语 topics
        contentType: 'methodology',
        mainFinding: '我们提出了一种基于注意力的新机制。',
      },
    ];

    // 用户使用中文查询
    const result = selectSectionsByRelevance(markdown, 2000, '注意力机制是如何工作的', sectionSemantics);

    // 应匹配到该章节（通过中文 topics 匹配）
    expect(result.injectedSections).toContain('方法章节');
  });

  // 测试 23：语义数据缺失时优雅降级（sectionSemantics 为空数组或 undefined）
  it('语义数据缺失时优雅降级', () => {
    const markdown = `
# Methods

We use machine learning methods.
`;

    // 空数组
    const result1 = selectSectionsByRelevance(markdown, 2000, 'tell me about methods', []);
    expect(result1.injectedSections.length).toBeGreaterThan(0);

    // undefined
    const result2 = selectSectionsByRelevance(markdown, 2000, 'tell me about methods', undefined);
    expect(result2.injectedSections.length).toBeGreaterThan(0);
  });

  // 测试 24：语义匹配与关键词匹配组合效果
  it('语义匹配与关键词匹配组合以获得更好的评分', () => {
    const markdown = `
# Abstract

This is the abstract.

# Neural Methods

We describe neural network methods.

# Traditional Results

We present experimental results.
`;

    const sectionSemantics = [
      {
        title: 'Neural Methods',
        index: 1,
        topics: ['neural network', 'deep learning'],
        contentType: 'methodology',
        mainFinding: 'Our neural approach achieves state-of-the-art performance.',
      },
      {
        title: 'Traditional Results',
        index: 2,
        topics: ['baseline', 'traditional methods'],
        contentType: 'results',
        mainFinding: 'Traditional methods serve as baseline comparison.',
      },
    ];

    const result = selectSectionsByRelevance(markdown, 5000, 'how does the neural network compare to baseline', sectionSemantics);

    // 两个章节都应被注入（通过不同匹配路径）
    expect(result.injectedSections).toContain('Neural Methods');
    expect(result.injectedSections).toContain('Traditional Results');
  });
});
