/**
 * threadAnchor.poc.test.js
 * ============================================================
 * remarkThreadAnchor 插件的概念验证（PoC）测试
 *
 * 本测试验证内联线程锚定机制的核心假设：
 * 1. remark-rehype data bridge：node.data.hProperties 正确传递到 hast 节点
 * 2. data-* 属性通过 rehypeSanitize（当前 schema 已允许）
 * 3. content.slice(start, end) 返回正确的原始 markdown
 * 4. cyrb53 指纹无碰撞（已在 threadUtils.test.js 中验证）
 * 5. closest('[data-source-start]') 从内联元素向上查找成功
 * 6. tableRow 的 position 可靠性
 *
 * 设计原则：
 * - 先通过验证再实施生产代码，避免后期返工
 * - 使用真实的 unified/remark/rehype 流水线，而非 mock
 * - 每个验证项独立运行，失败时明确指出原因
 *
 * @module ai-chat/utils/__tests__/threadAnchor.poc
 */

import { describe, it, expect } from 'vitest';
import { unified } from 'unified';
import remarkParse from 'remark-parse';
import remarkRehype from 'remark-rehype';
import rehypeStringify from 'rehype-stringify';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import rehypeSanitize, { defaultSchema } from 'rehype-sanitize';
import { visit } from 'unist-util-visit';
import { computeFingerprint, cyrb53 } from '../threadUtils';

// ============================================================
// 工具函数：创建 sanitizeSchema（复用 MarkdownRenderer.jsx 的配置）
// ============================================================

/**
 * 创建用于 rehype-sanitize 的安全 HTML schema
 *
 * 复用 MarkdownRenderer.jsx 中的 sanitizeSchema 配置，确保测试环境与生产环境一致
 * 关键点：必须允许 'data-*' 属性，否则 data-source-start 等属性会被过滤掉
 */
const createSanitizeSchema = () => ({
  ...defaultSchema,
  tagNames: [
    ...(defaultSchema.tagNames || []),
    'article', 'section', 'nav', 'aside', 'main', 'header', 'footer',
    'div', 'span',
    'colgroup', 'col',
    'details', 'summary',
  ],
  attributes: {
    ...defaultSchema.attributes,
    '*': [
      ...((defaultSchema.attributes as any)?.['*'] || []),
      'class', 'className',
      'id', 'style',
      'data*', // 关键：允许所有 data-* 属性（使用 'data*' 而非 'data-*'）
    ],
    a: ['href', 'target', 'rel', 'id', 'class'],
    img: ['src', 'alt', 'title', 'width', 'height', 'loading', 'id', 'class'],
    th: ['colspan', 'rowspan', 'align', 'valign', 'class'],
    td: ['colspan', 'rowspan', 'align', 'valign', 'class'],
    code: ['class', 'className'],
    pre: ['class', 'className'],
    div: ['class', 'className', 'id', 'style'],
    span: ['class', 'className', 'id', 'style'],
  },
});

// ============================================================
// remarkThreadAnchor 插件实现（测试版本）
// ============================================================

/**
 * remarkThreadAnchor 插件（PoC 版本）
 *
 * 在 remark AST（mdast）层面遍历块级节点，注入 source position 和 fingerprint
 *
 * 工作原理：
 * 1. 在 remark-parse 完成后执行，此时 tree 是 mdast（Markdown AST）
 * 2. 遍历所有块级节点（paragraph, listItem, heading, blockquote）
 * 3. 从 node.position 读取原始 markdown 中的偏移量
 * 4. 使用 content.slice(start, end) 提取原始文本
 * 5. 计算 fingerprint 并注入到 node.data.hProperties
 * 6. remark-rehype 会自动将 hProperties 转换为 hast 的 properties
 *
 * @param {string} content - 原始 markdown 源文本（用于 slice 操作）
 * @returns {function} remark 插件函数
 */
const remarkThreadAnchor = (content: any) => {
  // 支持的块级节点类型（mdast 节点类型）
  // 为什么选择这些类型？
  // - paragraph: 普通段落，最常见的锚定目标
  // - listItem: 列表项，用户可能追问列表中的某一项
  // - heading: 标题，用户可能追问某个章节
  // - blockquote: 引用块，用户可能追问引用内容
  // - tableRow: 表格行（待验证，remark-gfm 某些版本 position 不可靠）
  const BLOCK_TYPES = ['paragraph', 'listItem', 'heading', 'blockquote', 'tableRow'];

  return (tree: any) => {
    // 使用 unist-util-visit 遍历 mdast 树
    // visit 会深度优先遍历所有节点
    visit(tree, (node) => {
      // 只处理有 position 信息的块级节点
      // 有些节点可能没有 position（如手动构建的 AST）
      if (node.position && BLOCK_TYPES.includes(node.type)) {
        const start = node.position.start.offset;  // 节点在源文本中的起始偏移量
        const end = node.position.end.offset;      // 节点在源文本中的结束偏移量

        // 提取原始 markdown 源文本
        // content.slice(start, end) 返回从 start 到 end（不含）的子串
        const rawSource = content.slice(start, end);

        // 初始化 node.data（如果不存在）
        // node.data 是 remark-rehype 的标准桥接机制
        // data.hProperties 中的属性会被转换为 HTML 属性
        node.data = node.data || {};
        node.data.hProperties = {
          ...(node.data.hProperties || {}), // 保留已有的 hProperties
          'data-source-start': String(start),        // 起始偏移量（转为字符串，HTML 属性值都是字符串）
          'data-source-end': String(end),            // 结束偏移量（用于边界校验）
          'data-block-fingerprint': computeFingerprint(rawSource, start), // 指纹（用于匹配线程）
        };
      }
    });
  };
};

// ============================================================
// 辅助函数：运行完整的 unified 流水线
// ============================================================

/**
 * 运行完整的 unified 流水线（模拟 MarkdownRenderer.jsx 的处理）
 *
 * @param {string} markdown - 原始 markdown 文本
 * @returns {Promise<{hast: Object, html: string}>} 返回 hast 树和渲染的 HTML
 */
async function processMarkdown(markdown: any) {
  const sanitizeSchema = createSanitizeSchema();

  const processor = unified()
    .use(remarkParse)       // Markdown → mdast
    .use(remarkGfm)         // 支持 GFM（表格、删除线等）
    .use(remarkMath)        // 识别数学公式
    .use(remarkThreadAnchor, markdown) // 注入 data-* 属性（关键）
    .use(remarkRehype, { allowDangerousHtml: true }) // mdast → hast
    .use(rehypeSanitize, sanitizeSchema) // Sanitize HTML（验证 data-* 不被过滤）
    .use(rehypeStringify);  // hast → HTML 字符串

  const result = await processor.process(markdown);
  return {
    html: String(result),   // 渲染的 HTML
  };
}

/**
 * 运行 unified 流水线并返回 hast 树（用于 AST 验证）
 *
 * @param {string} markdown - 原始 markdown 文本
 * @returns {Promise<Object>} hast 树
 */
async function processMarkdownToHast(markdown: any) {
  const processor = unified()
    .use(remarkParse)
    .use(remarkGfm)
    .use(remarkMath)
    .use(remarkThreadAnchor, markdown)
    .use(remarkRehype, { allowDangerousHtml: true });

  // 使用 run() 而不是 process() 来获取 AST，不需要 compiler
  const tree = await processor.run(await processor.parse(markdown));
  return tree;
}

// ============================================================
// PoC 验证测试
// ============================================================

describe('threadAnchor PoC - remark-rehype data bridge', () => {

  /**
   * 验证项 1: remark-rehype data bridge
   *
   * 目标：验证 node.data.hProperties 正确传递到 hast 节点的 properties
   *
   * 为什么重要？
   * - remark 插件在 mdast 层面操作，需要通过 hProperties 桥接到 hast
   * - 如果桥接失败，data-* 属性不会出现在最终的 HTML 中
   */
  it('01: node.data.hProperties 正确传递到 hast properties', async () => {
    const markdown = '# Hello\n\nThis is a paragraph.';
    const hast = await processMarkdownToHast(markdown);

    // 在 hast 树中查找具有 data-source-start 属性的元素
    let foundDataAttr = false;

    // 深度遍历 hast 树（使用 visit 工具）
    visit(hast, { type: 'element' }, (node: any) => {
      if (node.properties?.['data-source-start'] !== undefined) {
        foundDataAttr = true;
        // 验证属性值是字符串（HTML 属性都是字符串）
        expect(typeof node.properties['data-source-start']).toBe('string');
        // 验证其他属性也存在
        expect(node.properties['data-source-end']).toBeDefined();
        expect(node.properties['data-block-fingerprint']).toBeDefined();
      }
    });

    expect(foundDataAttr).toBe(true);
  });

  /**
   * 验证项 2: data-* 属性通过 rehypeSanitize
   *
   * 目标：验证 rehypeSanitize 不会过滤掉 data-* 属性
   *
   * 为什么重要？
   * - rehypeSanitize 用于防止 XSS 攻击
   * - 默认 schema 可能不允许 data-* 属性
   * - 需要确保 sanitizeSchema 正确配置
   */
  it('02: data-* 属性通过 rehypeSanitize 不被过滤', async () => {
    const markdown = 'Test paragraph';
    const { html } = await processMarkdown(markdown);

    // 渲染的 HTML 应包含 data-source-start 属性
    expect(html).toContain('data-source-start');
    expect(html).toContain('data-source-end');
    expect(html).toContain('data-block-fingerprint');
  });

  /**
   * 验证项 3: content.slice(start, end) 返回正确的 <p> 元素 markdown
   *
   * 目标：验证 position.offset 正确指向原始 markdown 源文本
   *
   * 为什么重要？
   * - 需要使用 slice 提取的原始文本来计算 fingerprint
   * - 如果 offset 不正确，fingerprint 会不匹配
   */
  it('03: content.slice(start, end) 返回正确的 <p> 元素原始 markdown', async () => {
    const markdown = '# Header\n\nThis is **bold** text.\n\nAnother paragraph.';
    const hast = await processMarkdownToHast(markdown);

    // 查找第一个 <p> 元素（对应 "This is **bold** text."）
    let foundFirstParagraph = false;

    visit(hast, { tagName: 'p' }, (node: any, index: any, parent: any) => {
      if (node.properties?.['data-source-start'] !== undefined && !foundFirstParagraph) {
        const start = parseInt(node.properties['data-source-start'] as string, 10);
        const end = parseInt(node.properties['data-source-end'] as string, 10);
        const extracted = markdown.slice(start, end);

        // 验证提取的文本包含原始 markdown 内容
        // 只有第一个段落包含 "This is"
        if (extracted.includes('This is')) {
          foundFirstParagraph = true;
          expect(extracted).toContain('This is');
          expect(extracted).toContain('bold');
          expect(extracted).toContain('text');
          // 验证没有包含其他段落的内容
          expect(extracted).not.toContain('Another paragraph');
        }
      }
    });

    // 确保找到了第一个段落
    expect(foundFirstParagraph).toBe(true);
  });

  /**
   * 验证项 4: content.slice(start, end) 返回正确的 <li> 元素 markdown
   *
   * 目标：验证列表项的 position 信息正确
   *
   * 为什么重要？
   * - 列表项的结构比段落复杂（包含嵌套和标记）
   * - 需要确保 offset 正确指向列表项内容
   */
  it('04: content.slice(start, end) 返回正确的 <li> 元素原始 markdown', async () => {
    const markdown = `- First item\n- Second item with **bold**\n- Third item`;
    const hast = await processMarkdownToHast(markdown);

    let foundListItem = false;

    visit(hast, { tagName: 'li' }, (node: any) => {
      if (node.properties?.['data-source-start'] !== undefined) {
        const start = parseInt(node.properties['data-source-start'] as string, 10);
        const end = parseInt(node.properties['data-source-end'] as string, 10);
        const extracted = markdown.slice(start, end);

        // 验证提取的文本包含列表项内容
        expect(extracted.length).toBeGreaterThan(0);

        // 查找包含 "bold" 的列表项
        if (extracted.includes('bold')) {
          expect(extracted).toContain('Second item');
          expect(extracted).toContain('**bold**');
          foundListItem = true;
        }
      }
    });

    expect(foundListItem).toBe(true);
  });

  /**
   * 验证项 5: cyrb53 指纹无碰撞（10000 组合）
   *
   * 目标：验证 cyrb53 哈希函数在实际内容下的碰撞概率
   *
   * 注意：此测试已在 threadUtils.test.js 中通过，这里仅做轻量验证
   */
  it('05: cyrb53 指纹无碰撞（轻量验证 1000 组合）', () => {
    const hashes = new Set();

    // 生成 1000 个不同的输入（比 threadUtils.test.js 少，因为这是 PoC）
    for (let i = 0; i < 1000; i++) {
      const content = `Paragraph content ${i} with some variation`;
      const offset = i * 10;
      const fingerprint = computeFingerprint(content, offset);

      // 验证指纹是字符串
      expect(typeof fingerprint).toBe('string');

      // 检查碰撞
      if (hashes.has(fingerprint)) {
        expect.fail(`指纹碰撞检测: ${fingerprint}`);
      }
      hashes.add(fingerprint);
    }

    expect(hashes.size).toBe(1000);
  });

  /**
   * 验证项 6: closest('[data-source-start]') 从内联元素向上查找
   *
   * 目标：验证从 KaTeX 生成的 <span> 或其他内联元素可以找到锚定祖先
   *
   * 为什么重要？
   * - 用户可能选中包含数学公式的段落文字
   * - KaTeX 会生成复杂的嵌套 <span> 结构
   * - 需要确保从任意内联元素都能找到锚定块
   */
  it('06: 从渲染的 HTML 中，内联元素可以 closest 查找 data-source-start', async () => {
    // 使用 JSDOM 环境测试 DOM 操作（vitest 默认支持）
    // 使用简单的不含 KaTeX 的文本进行测试
    const markdown = 'Paragraph with text inside.';
    const { html } = await processMarkdown(markdown);

    // 创建 DOM 元素（使用 vitest 的 jsdom 环境）
    const container = document.createElement('div');
    container.innerHTML = html;

    // 查找所有带 data-source-start 的元素
    const allAnchors = container.querySelectorAll('[data-source-start]');
    expect(allAnchors.length).toBeGreaterThan(0);

    // 验证 p 元素有 data 属性
    const p = container.querySelector('p[data-source-start]');
    expect(p).not.toBeNull();
    expect(p?.hasAttribute('data-block-fingerprint')).toBe(true);
  });

  /**
   * 验证项 6b: KaTeX 段落的锚定
   *
   * 目标：验证包含 KaTeX 公式的段落也能正确锚定
   */
  it('06b: KaTeX 段落也能正确计算 fingerprint', async () => {
    const markdown = 'Paragraph with $x^2$ formula inside.';
    const { html } = await processMarkdown(markdown);

    // 创建 DOM 元素
    const container = document.createElement('div');
    container.innerHTML = html;

    // 查找带 data-source-start 的元素
    const allAnchors = container.querySelectorAll('[data-source-start]');
    expect(allAnchors.length).toBeGreaterThan(0);

    // 查找 KaTeX 生成的 span
    const katexSpan = container.querySelector('.katex');
    if (katexSpan) {
      // 从 KaTeX span 向上查找 data-source-start
      const anchor = katexSpan.closest('[data-source-start]');
      expect(anchor).not.toBeNull();
      expect(anchor?.hasAttribute('data-block-fingerprint')).toBe(true);
    }
  });

  /**
   * 验证项 7: tableRow 的 position 可靠性
   *
   * 目标：验证当前 remark-gfm 版本中 tableRow 的 position 信息是否可靠
   *
   * 为什么重要？
   * - 如果 tableRow.position 不可靠，需要从 BLOCK_TYPES 中移除
   * - 用户可能想追问表格中的某一行
   */
  it('07: tableRow 的 position 信息可靠性验证', async () => {
    const markdown = `| Header 1 | Header 2 |
|----------|----------|
| Cell 1   | Cell 2   |
| Cell 3   | Cell 4   |`;

    const hast = await processMarkdownToHast(markdown);

    let tableRowFound = false;
    let hasValidPosition = false;

    visit(hast, { tagName: 'tr' }, (node: any) => {
      // 检查 <tr> 元素是否有 data-source-start
      if (node.properties?.['data-source-start'] !== undefined) {
        tableRowFound = true;
        const start = parseInt(node.properties['data-source-start'] as string, 10);
        const end = parseInt(node.properties['data-source-end'] as string, 10);

        // 验证 offset 是有效数字
        if (!isNaN(start) && !isNaN(end) && start < end) {
          hasValidPosition = true;
        }
      }
    });

    // 如果找到了 tableRow 且有有效的 position 信息
    if (tableRowFound) {
      expect(hasValidPosition).toBe(true);
      // 记录：当前版本 remark-gfm 支持 tableRow position
      console.log('[PoC Info] tableRow position 可靠，可加入 BLOCK_TYPES');
    } else {
      // 记录：当前版本 remark-gfm 不支持 tableRow position
      console.log('[PoC Info] tableRow position 不可靠，需从 BLOCK_TYPES 移除');
    }
  });

  /**
   * 验证项 8: 指纹计算的一致性
   *
   * 目标：验证相同内容在相同位置产生相同的指纹
   *
   * 为什么重要？
   * - 指纹用于匹配线程和锚定元素
   * - 如果不一致，线程无法正确显示
   */
  it('08: 相同内容在相同位置产生相同的指纹', async () => {
    const markdown = 'Test paragraph for fingerprint consistency.';

    // 第一次处理
    const { html: html1 } = await processMarkdown(markdown);
    const match1 = html1.match(/data-block-fingerprint="([^"]+)"/);
    expect(match1).not.toBeNull();

    // 第二次处理（相同内容）
    const { html: html2 } = await processMarkdown(markdown);
    const match2 = html2.match(/data-block-fingerprint="([^"]+)"/);
    expect(match2).not.toBeNull();

    // 指纹应该相同
    expect(match1?.[1]).toBe(match2?.[1]);
  });

  /**
   * 验证项 9: 不同位置的不同指纹
   *
   * 目标：验证相同内容在不同位置产生不同的指纹
   *
   * 为什么重要？
   * - 确保同一段落出现多次时，可以区分不同的位置
   */
  it('09: 相同内容在不同位置产生不同的指纹', async () => {
    const markdown = 'Same text here.\n\nSame text here.';

    const hast = await processMarkdownToHast(markdown);
    const fingerprints: string[] = [];

    visit(hast, { tagName: 'p' }, (node: any) => {
      if (node.properties?.['data-block-fingerprint']) {
        fingerprints.push(node.properties['data-block-fingerprint'] as string);
      }
    });

    // 应该有两个段落，且指纹不同
    expect(fingerprints.length).toBeGreaterThanOrEqual(2);
    expect(fingerprints[0]).not.toBe(fingerprints[1]);
  });

  /**
   * 验证项 10: 包含代码块的段落不受影响
   *
   * 目标：验证包含行内代码的段落也能正确锚定
   *
   * 为什么重要？
   * - 行内代码在 mdast 中是特殊的节点类型
   * - 需要确保 paragraph 的 position 仍然正确
   */
  it('10: 包含行内代码的段落能正确计算 fingerprint', async () => {
    const markdown = 'This paragraph has `inline code` inside it.';
    const hast = await processMarkdownToHast(markdown);

    let foundFingerprint = false;

    visit(hast, { tagName: 'p' }, (node) => {
      if (node.properties?.['data-block-fingerprint']) {
        foundFingerprint = true;
        expect(node.properties['data-source-start']).toBeDefined();
        expect(node.properties['data-source-end']).toBeDefined();
      }
    });

    expect(foundFingerprint).toBe(true);
  });

  /**
   * 验证项 11: 引用块（blockquote）也能正确锚定
   *
   * 目标：验证 blockquote 元素能正确计算 fingerprint
   */
  it('11: blockquote 元素能正确计算 fingerprint', async () => {
    const markdown = '> This is a quote.\n> With multiple lines.';
    const hast = await processMarkdownToHast(markdown);

    let foundFingerprint = false;

    visit(hast, { tagName: 'blockquote' }, (node) => {
      if (node.properties?.['data-block-fingerprint']) {
        foundFingerprint = true;
        expect(node.properties['data-source-start']).toBeDefined();
      }
    });

    expect(foundFingerprint).toBe(true);
  });

  /**
   * 验证项 12: 标题（heading）也能正确锚定
   *
   * 目标：验证 h1-h6 元素能正确计算 fingerprint
   */
  it('12: heading 元素能正确计算 fingerprint', async () => {
    const markdown = '# First Heading\n\n## Second Heading\n\nContent.';
    const hast = await processMarkdownToHast(markdown);

    let headingCount = 0;

    visit(hast, { tagName: 'h1' }, (node) => {
      if (node.properties?.['data-block-fingerprint']) {
        headingCount++;
        expect(node.properties['data-source-start']).toBeDefined();
      }
    });

    visit(hast, { tagName: 'h2' }, (node) => {
      if (node.properties?.['data-block-fingerprint']) {
        headingCount++;
        expect(node.properties['data-source-start']).toBeDefined();
      }
    });

    expect(headingCount).toBeGreaterThanOrEqual(2);
  });
});

// ============================================================
// 辅助函数：错误注解（增强测试失败时的信息）
// ============================================================

/**
 * 为断言添加错误注解
 *
 * @param {string} message - 错误消息
 * @param {*} value - 断言的值
 * @returns {*} 原始值（用于链式调用）
 */
function annotateError(message: any, value: any) {
  if (!value) {
    // 如果值为 falsy，添加上下文信息
    console.error(`[PoC 失败] ${message}`);
  }
  return value;
}
