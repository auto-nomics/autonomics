/**
 * MarkdownRenderer.jsx
 * ============================================================
 * Markdown 渲染组件（支持内联线程）
 *
 * 本组件将 AI 返回的 Markdown 文本转换为格式化的 HTML 并展示。
 * 在原有功能基础上，现在支持内联线程（Inline Thread）功能：
 * - 每个块级元素（段落、列表项、标题、引用）都带有指纹标识
 * - 线程可以根据指纹附加到对应的块级元素下方
 * - 无线程时零开销，有线程时按指纹匹配注入
 *
 * 支持的功能：
 * - GitHub Flavored Markdown（GFM）：表格、删除线、任务列表等
 * - LaTeX 数学公式：行内公式 ($...$ 或 \(...\)) 和块级公式 ($$...$$ 或 \[...\])
 * - 代码语法高亮：自动检测语言并着色
 * - 原始 HTML 标签支持
 * - 图片点击放大预览
 * - 链接在新标签页打开
 * - 搜索关键词高亮：在渲染后的 HTML 文本中标记匹配的关键词
 * - 内联线程：在块级元素下方显示线程 UI
 *
 * ============================================================
 */

import React, { useMemo, useRef, memo } from 'react';
// 导入 React Markdown 组件（核心 Markdown 渲染引擎）
import ReactMarkdown from 'react-markdown';
// 导入聊天图片灯箱组件（用于 Markdown 中的图片点击放大）
import ChatImageLightbox from './ChatImageLightbox';
// 导入内联线程组件（用于在块级元素下方显示追问线程）
import InlineThread, { ThreadGroupEntry } from './InlineThread';
// 导入 remark 插件（Markdown AST 层面的转换）
import remarkGfm from 'remark-gfm';          // GitHub Flavored Markdown 支持
import remarkMath from 'remark-math';        // 数学公式语法识别
// 导入自定义 remark 插件（注入块级元素指纹）
import remarkThreadAnchor, { rehypeRestoreThreadAnchors } from '../utils/remarkThreadAnchor';
// 导入 LaTeX 分隔符预处理器（把 \(...\) / \[...\] 转换为 $...$ / $$...$$）
import { preprocessLatexDelimiters } from '../utils/latexDelimiterPreprocessor';
// 导入 rehype 插件（HTML 层面的转换）
import rehypeKatex from 'rehype-katex';          // LaTeX 公式渲染
import rehypeRaw from 'rehype-raw';              // 原始 HTML 标签支持
import rehypeSanitize, { defaultSchema } from 'rehype-sanitize'; // HTML 安全 sanitization，防止 XSS 攻击
// 导入代码高亮和数学公式的 CSS 样式
import 'katex/dist/katex.min.css';        // KaTeX 数学公式样式
// highlight.js 按需加载：只导入常用语言，减少打包体积
import rehypeHighlight from 'rehype-highlight';  // 代码语法高亮
// 从 highlight.js/lib/languages 按需导入常用语言
import python from 'highlight.js/lib/languages/python';
import javascript from 'highlight.js/lib/languages/javascript';
import typescript from 'highlight.js/lib/languages/typescript';
import rust from 'highlight.js/lib/languages/rust';
import json from 'highlight.js/lib/languages/json';
import bash from 'highlight.js/lib/languages/bash';
import sql from 'highlight.js/lib/languages/sql';
import css from 'highlight.js/lib/languages/css';
import xml from 'highlight.js/lib/languages/xml';
import markdown from 'highlight.js/lib/languages/markdown';

/**
 * 创建用于 rehype-sanitize 的安全 HTML schema
 *
 * 基于 defaultSchema，扩展允许的标签和属性以支持常见的安全 HTML：
 * - 表格相关: colgroup, col, thead, tbody, tfoot
 * - 交互元素: details, summary
 * - 通用容器: div, span（允许 class 属性用于样式）
 * - 语义标签: article, section, nav, aside, main, header, footer
 * - 线程相关: data-* 属性（用于块级元素指纹）
 *
 * 安全特性：
 * - 仍然阻止危险元素: script, iframe, object, embed, form, input, button
 * - 仍然阻止危险属性: on* 事件处理器, javascript: URLs
 * - 允许安全的 class, id, style, data* 属性
 */
const sanitizeSchema = {
  ...defaultSchema,
  tagNames: [
    ...(defaultSchema.tagNames || []),
    // 额外允许的语义化标签
    'article', 'section', 'nav', 'aside', 'main', 'header', 'footer',
    'div', 'span',
    'colgroup', 'col',
    // details/summary 用于可折叠内容
    'details', 'summary',
  ],
  attributes: {
    ...(defaultSchema.attributes || {}),
    // 为特定标签扩展允许的属性
    '*': [
      ...((defaultSchema.attributes || {})['*'] || []),
      'class', 'className', // 允许 class 用于样式绑定
      'id', // 允许基本样式
      'data*', // 允许所有 data-* 属性（关键：线程锚定使用）
    ],
    // a 标签允许 href（但 javascript: 协议会被默认阻止）
    a: ['href', 'target', 'rel', 'id', 'class'],
    // img 标签允许更多属性
    img: ['src', 'alt', 'title', 'width', 'height', 'loading', 'id', 'class'],
    // th/td 允许表格属性
    th: ['colspan', 'rowspan', 'align', 'valign', 'class'],
    td: ['colspan', 'rowspan', 'align', 'valign', 'class'],
    // code/pre 允许语言标识类
    code: ['class', 'className'],
    pre: ['class', 'className'],
    // div/span 容器：允许 style 属性。
    // KaTeX 输出大量依赖 inline style（strut 高度、vlist 垂直偏移、frac-line 横线宽度、
    // mspace 间距等）撑起公式布局，剥掉 style 会导致公式坍缩重叠。
    // 现代浏览器（含 Tauri WebView2/WebKitGTK）的 CSS 上下文已不支持 javascript: 协议，
    // 这里允许 style 不会引入有意义的 XSS 面。
    div: ['class', 'className', 'id', 'style'],
    span: ['class', 'className', 'id', 'style'],
  },
};

/**
 * 创建自定义的 highlight 语言配置
 *
 * 使用按需导入的 highlight.js 语言模块，只包含常用语言，减少打包体积。
 */
function createHighlightLanguages() {
  return {
    python,
    javascript,
    typescript,
    rust,
    json,
    bash,
    sql,
    css,
    xml,
    markdown,
  };
}

/**
 * 基础 rehype 插件数组（不依赖组件 props 或 state）
 *
 * 为什么作为模块级常量？
 * - 这些插件配置在每次渲染时都是相同的
 * - 作为常量可以避免每次组件渲染时重建数组
 * - 只有 createRehypeHighlightKeyword 插件需要根据 searchKeyword 动态添加
 *
 * 插件顺序很重要：
 * 1. rehypeKatex - 先渲染 LaTeX 公式
 * 2. rehypeHighlight - 再进行代码高亮（使用按需加载的语言子集）
 * 3. rehypeRaw - 允许原始 HTML 标签
 * 4. rehypeSanitize - 最后 sanitization HTML，防止 XSS 攻击
 */
const BASE_REHYPE_PLUGINS: any[] = [
  [rehypeKatex, { strict: 'ignore' }],
  [rehypeHighlight, { languages: createHighlightLanguages() }],
  rehypeRaw,
  [rehypeSanitize, sanitizeSchema],
];

/**
 * 线程合并显示阈值
 * 当同一段落的线程数量达到此阈值时，使用 ThreadGroupEntry 合并显示
 */
const THREAD_GROUP_THRESHOLD = 3;

/**
 * 创建 rehype 插件：在 HTML AST（hast 树）中的文本节点里高亮搜索关键词
 *
 * 工作原理：
 * 1. 在所有其他 rehype 插件（KaTeX、代码高亮、原始 HTML）处理完毕后执行
 * 2. 深度遍历 hast 树，找到所有文本节点（type: 'text'）
 * 3. 对包含关键词的文本节点进行正则拆分
 * 4. 将匹配的部分包裹在 <mark class="chat-search-highlight"> 元素中
 * 5. 复用 ChatPanel 中已有的 CSS 类 .chat-search-highlight（黄色半透明背景）
 *
 * 为什么使用 rehype 插件而不是 DOM 操作（useEffect + innerHTML）？
 * - rehype 插件在虚拟 AST 层面操作，与 React 渲染流水线完全兼容
 * - 不存在 React 虚拟 DOM 与真实 DOM 不同步的风险
 * - 性能更优：AST 遍历比 DOM 遍历更轻量
 *
 * @param {string} keyword - 要高亮的搜索关键词（空字符串或 undefined 时插件不执行任何操作）
 * @returns {function} rehype 转换函数，接收 hast 树并原地修改
 */
function createRehypeHighlightKeyword(keyword: string) {
    // 关键词为空或仅含空白字符时，返回空操作的 rehype 插件
    if (!keyword?.trim()) {
        // 返回标准 rehype 插件格式：(options) => transformer(tree)
        // 外层函数接收 options（忽略），内层函数接收 hast tree（不修改直接返回）
        return () => () => {}; // 空操作（no-op）插件 → 空操作 transformer
    }

    // 去除首尾空白后的关键词（用于大小写不敏感比较）
    const kw = keyword.trim(); // 标准化关键词
    // 转义关键词中的正则特殊字符（如 C++ 中的 +，user@email 中的 . 等）
    // 防止这些字符被当作正则量词或元字符导致错误
    const escaped = kw.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'); // 逐字符转义
    // 构建全局、大小写不敏感的捕获正则
    // 捕获组 () 用于 split 后保留匹配文本（split 会丢弃非捕获组匹配）
    const regex = new RegExp(`(${escaped})`, 'gi'); // g=全局匹配，i=忽略大小写

    /**
     * 递归处理 hast 树节点
     *
     * 处理策略（两阶段）：
     * 阶段1：先深度优先递归处理所有子元素节点（element 类型）
     *        这样可以确保内层元素中的文本先被处理
     * 阶段2：再处理当前层级的文本子节点（text 类型）
     *        将包含关键词的文本拆分为 text + <mark> + text 序列
     *
     * 为什么分两个阶段？
     * - 阶段1处理的是原始子节点，不包含我们新创建的 <mark> 元素
     * - 阶段2创建的 <mark> 元素不会被递归处理，避免了无限递归
     * - 例如 <p>Hello <strong>wor</strong>ld</p> 中搜索 "world"
     *   虽然 "world" 跨越了两个节点无法匹配，但 "wor" 可以在 <strong> 内被独立匹配
     *
     * @param {Object} node - hast 树节点（element 或 root 类型）
     */
    function processNode(node: any) {
        // 没有子节点的节点（如文本节点、注释节点）不需要处理
        if (!node.children) return; // 跳过无子节点的节点

        // ===== 阶段1：递归处理现有元素子节点 =====
        // 遍历当前节点的所有子节点
        for (const child of node.children) {
            // 只递归处理元素类型（type: 'element'）的子节点
            // 文本节点（type: 'text'）在阶段2处理
            if (child.type === 'element') {
                processNode(child); // 深度优先递归处理子元素
            }
        }

        // ===== 阶段2：处理当前层级的文本子节点 =====
        // 构建新的子节点数组，将匹配的文本节点替换为 text + <mark> 序列
        const newChildren = []; // 新的子节点数组
        // 遍历当前节点的所有子节点（此时元素子节点已被阶段1递归处理过）
        for (const child of node.children) {
            // 检查是否为文本节点，且其内容包含关键词（大小写不敏感）
            if (child.type === 'text' && child.value && child.value.toLowerCase().includes(kw.toLowerCase())) {
                // 文本节点包含关键词，需要拆分并高亮
                // split(regex) 会按匹配位置拆分字符串，匹配文本保留在结果数组中
                // 例如 "hello world hello".split(/(hello)/gi) → ["", "hello", " world ", "hello", ""]
                const parts = child.value.split(regex); // 拆分为交替的非匹配/匹配片段
                // 遍历拆分后的每个片段
                for (const part of parts) {
                    // 检查当前片段是否与关键词匹配（大小写不敏感）
                    if (part.toLowerCase() === kw.toLowerCase()) {
                        // 匹配片段：创建 <mark> 元素节点包裹匹配文本
                        newChildren.push({
                            type: 'element',               // hast 元素节点类型
                            tagName: 'mark',               // HTML <mark> 标签（语义化高亮标记）
                            properties: {                  // HTML 属性
                                className: 'chat-search-highlight' // 复用 ChatPanel 中已有的高亮 CSS 类
                            },
                            children: [                    // <mark> 标签内的文本子节点
                                {
                                    type: 'text',          // 文本节点
                                    value: part            // 保留原始大小写的匹配文本
                                }
                            ]
                        });
                    } else if (part) {
                        // 非匹配片段（且不为空字符串）：保留为普通文本节点
                        newChildren.push({
                            type: 'text',  // 文本节点类型
                            value: part    // 原始文本（不需要高亮）
                        });
                    }
                    // 空字符串片段（split 可能产生）直接跳过，不添加到新数组
                }
            } else {
                // 非文本节点（元素节点）或不含关键词的文本节点：原样保留
                newChildren.push(child); // 不做修改，直接添加到新数组
            }
        }
        // 用处理后的新子节点数组替换原有的子节点数组
        node.children = newChildren; // 原地修改 AST 节点
    }

    // 返回标准格式的 rehype 插件函数
    // rehype/unified 的插件调用约定：
    //   use(plugin)       → plugin() 返回 transformer
    //   use(plugin, opts) → plugin(opts) 返回 transformer
    //   transformer(tree, vfile) 实际处理 hast 树
    // 因此返回值必须是 (options) => (tree) => { ... } 的两层函数结构
    return () => (tree: any) => { // 外层函数：接收 options（本插件不需要配置项，忽略）；内层函数：接收 hast tree
        processNode(tree); // 从根节点开始递归处理整棵树，原地修改 AST
    };
}

/**
 * 从 hast 节点中提取表格行（tr）的指纹，并匹配对应线程
 *
 * 用于 table 组件渲染追问线程 UI。因为 <tr> 不能直接作为线程锚定宿主
 * （在 <tbody> 内插入 <div> 会破坏表格布局），所以线程在 table 层级统一渲染。
 */
function extractTableThreadMatches(node: any, threadByFingerprint: Map<any, any>, threadsRef: any) {
    if (!threadByFingerprint || threadByFingerprint.size === 0) return [];

    const fingerprints: string[] = [];
    function visitHast(element: any) {
        if (!element) return;
        if (element.tagName === 'tr' && element.properties?.['data-block-fingerprint']) {
            fingerprints.push(String(element.properties['data-block-fingerprint']));
        }
        if (element.children) {
            for (const child of element.children) {
                if (child.type === 'element') visitHast(child);
            }
        }
    }
    visitHast(node);

    if (fingerprints.length === 0) return [];

    const matched: any[] = [];
    const seen = new Set<string>();
    for (const fp of fingerprints) {
        const threads: any[] = threadByFingerprint.get(fp) || [];
        for (const stale of threads) {
            if (seen.has(stale.id)) continue;
            seen.add(stale.id);
            const fresh = threadsRef?.current?.find((t: any) => t.id === stale.id);
            matched.push(fresh || stale);
        }
    }
    return matched;
}

/**
 * 创建线程感知的组件集合
 *
 * 此函数包装块级组件（p, li, h1-h6, blockquote），在匹配的块级元素后注入 InlineThread。
 *
 * 设计要点：
 * 1. createBlockComponent 必须在 useMemo 内部定义，否则每次渲染创建新组件引用导致 React 卸载/重挂载
 * 2. 无 threads prop 时直接返回原组件（零开销 passthrough）
 * 3. 从 props['data-block-fingerprint'] 读取指纹（无需运行时计算）
 * 4. 同一块级元素可附加多个线程（按 data-source-start offset 排序）
 * 5. Phase 6: 线程数 >= 3 时使用 ThreadGroupEntry 合并显示，减少 UI 噪音
 *
 * @param {Object} baseComponents - 原始组件集合（来自 ReactMarkdown 的 components prop）
 * @param {Map<string, Array>} threadByFingerprint - 指纹到线程数组的映射
 * @param {Function} onThreadAction - 线程操作回调函数，签名为 (type, threadId, ...args) => void
 * @param {Function} isThreadStreamingById - 判断指定线程是否在流式回复中 (threadId) => boolean（Phase 6：并行流式支持）
 * @param {string} chatSearchKeyword - 聊天搜索关键词，传递给 InlineThread 组件用于消息内高亮
 * @returns {Object} 包装后的组件集合
 */
function createThreadAwareComponents(baseComponents: any, threadByFingerprint: Map<any, any>, onThreadAction: any, isThreadStreamingById: any, chatSearchKeyword: any, threadsRef: any, isThreadCollapsed: any, onActivateThread: any, nestingLevel: any, onSubThreadAction: any, isSubThreadStreamingById: any, isSubThreadCollapsed: any) {
  // 无线程时直接返回原组件（零开销）
  if (!threadByFingerprint || threadByFingerprint.size === 0) {
    return baseComponents;
  }

  /**
   * 创建线程感知的块级组件
   *
   * @param {string} tagName - HTML 标签名（如 'p', 'li'）
   * @param {React.Component} BaseComponent - 原始组件
   * @returns {React.Component} 包装后的组件
   */
  const createBlockComponent = (tagName: any, BaseComponent: any) => {
    const BlockComponent = (props: any) => {
      // 从 props 中读取指纹（由 remarkThreadAnchor 注入）
      const fingerprint = props['data-block-fingerprint'];

      // 查找匹配该指纹的线程数组
      // 使用 threadByFingerprint 获取线程 ID，再从 threadsRef 读取最新数据
      // 这样即使 threadByFingerprint 被缓存（指纹未变），线程消息数据也是最新的
      const staleThreads = threadByFingerprint.get(String(fingerprint)) || [];
      const matchedThreads = staleThreads.map((stale: any) => {
        const fresh = threadsRef?.current?.find((t: any) => t.id === stale.id);
        return fresh || stale;
      });

      // 无匹配线程时直接渲染原内容
      if (matchedThreads.length === 0) {
        return <BaseComponent {...props} />;
      }

      /**
       * Phase 6: 多线程合并显示逻辑
       * - 线程数 < THREAD_GROUP_THRESHOLD (3): 逐个平铺显示 InlineThread
       * - 线程数 >= THREAD_GROUP_THRESHOLD (3): 使用 ThreadGroupEntry 合并显示
       *
       * 设计原因：
       * - 减少同一段落下方过多线程造成的 UI 噪音
       * - 用户按需展开查看所有追问，保持界面整洁
       * - 阈值设为 3 是基于 UX 考虑：1-2 个线程不会造成视觉干扰
       */
      const shouldGroupThreads = matchedThreads.length >= THREAD_GROUP_THRESHOLD;

      /**
       * 线程操作回调适配器
       *
       * ThreadGroupEntry 需要接收签名为 (type, threadId, ...args) 的回调
       * 此函数将 ThreadGroupEntry 的回调转发到父组件传入的 onThreadAction
       */
      const handleThreadAction = (type: string, threadId: string, ...args: any[]) => {
        onThreadAction?.(type, threadId, ...args);
      };

      return (
        <>
          {/* 渲染原始块级内容 */}
          <BaseComponent {...props} />

          {/* Phase 6: 根据线程数量选择渲染方式 */}
          {shouldGroupThreads ? (
            // 线程数 >= 3：使用 ThreadGroupEntry 合并显示
            <ThreadGroupEntry
              threads={matchedThreads}
              onToggle={(threadId) => handleThreadAction('toggle', threadId)}
              onDelete={(threadId) => handleThreadAction('delete', threadId)}
              isStreaming={false}
              chatSearchKeyword={chatSearchKeyword}
              isThreadStreaming={isThreadStreamingById}
              onActivate={onActivateThread}
              nestingLevel={nestingLevel}
              onSubThreadAction={onSubThreadAction}
              isSubThreadStreamingById={isSubThreadStreamingById}
              isSubThreadCollapsed={isSubThreadCollapsed}
            />
          ) : (
            // 线程数 < 3：逐个平铺显示 InlineThread
            matchedThreads.map((thread: any) => {
              return (
                <InlineThread
                  key={thread.id}
                  thread={thread}
                  collapsed={isThreadCollapsed(thread.id)}
                  onToggle={() => handleThreadAction('toggle', thread.id)}
                  onDelete={() => handleThreadAction('delete', thread.id)}
                  isStreaming={isThreadStreamingById(thread.id)} // Phase 6: 使用函数检查线程流式状态
                  chatSearchKeyword={chatSearchKeyword}
                  onActivate={onActivateThread}
                  nestingLevel={nestingLevel}
                  onSubThreadAction={onSubThreadAction}
                  isSubThreadStreamingById={isSubThreadStreamingById}
                  isSubThreadCollapsed={isSubThreadCollapsed}
                />
              );
            })
          )}
        </>
      );
    };

    return BlockComponent;
  };

  /**
   * 创建透传 props 的默认块级组件
   *
   * 关键：必须将 data-source-start、data-block-fingerprint 等属性透传到 DOM，
   * 否则 processTextSelection 中的 closest('[data-source-start]') 会找不到锚定元素，
   * 导致追问功能在线程创建后失效。
   *
   * 之前的写法 ({ children }) => <p>{children}</p> 会丢弃所有 props（包括 data-*），
   * 在无线程时 ReactMarkdown 默认渲染正常，但有线程时经过 BlockComponent 包装后
   * data 属性丢失。
   */
  const forwardProps = (Tag: any) => ({ node, children, ...rest }: any) => <Tag {...rest}>{children}</Tag>;

  // 创建包装后的组件集合
  return {
    ...baseComponents,
    // 包装所有块级元素（使用 forwardProps 确保 data-* 属性透传到 DOM）
    p: createBlockComponent('p', baseComponents.p || forwardProps('p')),
    li: createBlockComponent('li', baseComponents.li || forwardProps('li')),
    h1: createBlockComponent('h1', baseComponents.h1 || forwardProps('h1')),
    h2: createBlockComponent('h2', baseComponents.h2 || forwardProps('h2')),
    h3: createBlockComponent('h3', baseComponents.h3 || forwardProps('h3')),
    h4: createBlockComponent('h4', baseComponents.h4 || forwardProps('h4')),
    h5: createBlockComponent('h5', baseComponents.h5 || forwardProps('h5')),
    h6: createBlockComponent('h6', baseComponents.h6 || forwardProps('h6')),
    blockquote: createBlockComponent('blockquote', baseComponents.blockquote || forwardProps('blockquote')),
  };
}

/**
 * Markdown 渲染组件（支持内联线程）
 * 将 Markdown 字符串渲染为格式化的 HTML 内容
 *
 * Phase 6 优化：支持并行流式
 * - 接受 isThreadStreamingById 函数而非单个 streamingThreadId
 * - 每个线程可以独立判断自己的流式状态
 *
 * @param {Object} props
 * @param {string} props.content - 要渲染的 Markdown 文本内容
 * @param {string} [props.searchKeyword=''] - 聊天搜索关键词，提供后会在渲染的文本中高亮匹配的关键词
 * @param {Array} [props.threads=[]] - 线程数组，每个线程包含 anchorFingerprint 等信息
 * @param {Function} [props.onThreadAction] - 线程操作回调函数，签名为 (type, threadId, ...args) => void
 * @param {Function} [props.isThreadStreamingById] - 判断指定线程是否在流式回复中 (threadId) => boolean（Phase 6）
 */
const MarkdownRenderer = ({ content, searchKeyword = '', threads = [], onThreadAction, isThreadStreamingById = () => false as boolean, isThreadStreaming, isThreadCollapsed = () => false as boolean, onActivateThread = null as ((threadId: string) => void) | null, nestingLevel = 0, onSubThreadAction, isSubThreadStreamingById, isSubThreadCollapsed }: {
    content: string;
    searchKeyword?: string;
    threads?: any[];
    onThreadAction?: (type: string, threadId: string, ...args: any[]) => void;
    isThreadStreamingById?: (threadId: string) => boolean;
    isThreadStreaming?: any;
    isThreadCollapsed?: (threadId: string) => boolean;
    onActivateThread?: ((threadId: string) => void) | null;
    nestingLevel?: number;
    onSubThreadAction?: (type: string, threadId: string, ...args: any[]) => void;
    isSubThreadStreamingById?: (threadId: string) => boolean;
    isSubThreadCollapsed?: (threadId: string) => boolean;
}) => {
    // 共享的指纹映射 store（始终复用同一个 Map 对象）
    // remark 阶段写入，rehype 阶段读取。不能每次创建新 Map，
    // 否则 rehypePlugins useMemo 先于 remarkPlugins 执行时会捕获旧的引用。
    const fingerprintStoreRef = useRef(new Map());

    // 预处理 LaTeX 分隔符：把 \(...\) / \[...\] 转换为 $...$ / $$...$$
    // remark-math@6 不识别 LaTeX 风格分隔符，且 CommonMark 解析时会把
    // \( 当作转义的 ( 吞掉反斜杠，所以必须在解析前做字符串替换。
    // 同一份 processedContent 也会传给 remarkThreadAnchor，确保 position 对齐。
    const processedContent = useMemo(
        () => preprocessLatexDelimiters(content),
        [content],
    );

    // 线程数据的 ref：始终指向最新的 threads 数组
    // 用途：wrapper 组件通过 ref 读取最新线程消息数据，
    // 避免 useMemo 缓存导致线程消息更新后 UI 不刷新
    const threadsRef = useRef(threads);
    threadsRef.current = threads;

    // 构建 rehype 插件数组：基础插件 + 可选的关键词高亮插件
    const rehypePlugins = useMemo(() => {
        const plugins = [...BASE_REHYPE_PLUGINS];
        // 在 rehypeRaw 之后、rehypeSanitize 之前插入恢复插件
        const sanitizeIndex = plugins.findIndex(p => Array.isArray(p) && p[0] === rehypeSanitize);
        if (sanitizeIndex !== -1) {
            plugins.splice(sanitizeIndex, 0, [rehypeRestoreThreadAnchors, fingerprintStoreRef.current] as any);
        }
        if (searchKeyword?.trim()) {
            plugins.push(createRehypeHighlightKeyword(searchKeyword) as any);
        }
        return plugins;
    }, [searchKeyword, content]);

    /**
     * 构建 remark 插件数组
     */
    const remarkPlugins = useMemo(() => {
        // SAFETY: ref mutation in useMemo 体属于 "踩在规范边界" 的写法 —— React
        // 18 当前 sync 执行 useMemo, 但未来 React Compiler / Concurrent 下可能 lazy。
        // 这里 mutate 是必要的: rehype 插件持有同一个 Map 引用, clear 后由插件
        // 重新填充, 否则上一次 render 留下的指纹会泄漏到下一次。改用 useEffect
        // 会让 clear 在 commit 阶段才跑, 时机太晚 (ReactMarkdown 已经用过旧 Map
        // 渲染完)。如果未来 React 行为变化破坏此假设, 改成 ref.current = new Map()
        // + 同步 rehype 插件闭包变量即可。
        fingerprintStoreRef.current.clear();
        return [
            remarkGfm,
            remarkMath,
            // 传入 processedContent 而非原始 content，确保 position 与
            // ReactMarkdown 实际解析的字符串一致（指纹切片基于此字符串）
            [remarkThreadAnchor, processedContent, fingerprintStoreRef.current] as any,
        ];
    }, [processedContent]);

    /**
     * 构建指纹到线程数组的映射
     *
     * 设计要点：
     * - 使用 Map<string, Thread[]> 一对多映射（同一段落可有多个线程）
     * - 线程按 data-source-start offset 排序（确保按原文位置渲染）
     * - 依赖项为 threads 的指纹列表（线程内消息变化不触发重建）
     */
    const threadByFingerprint = useMemo(() => {
        // 无线程时返回空 Map
        if (!threads || threads.length === 0) {
            return new Map();
        }

        const map = new Map();

        // 按指纹分组线程
        for (const thread of threads as any[]) {
            const fingerprint = String(thread.anchorFingerprint);
            if (!map.has(fingerprint)) {
                map.set(fingerprint, []);
            }
            map.get(fingerprint).push(thread);
        }

        // 对每个指纹的线程数组按 data-source-start offset 排序
        // 确保线程按原文位置顺序渲染（用户可能乱序创建线程）
        for (const [fingerprint, threadList] of map.entries()) {
            threadList.sort((a: any, b: any) => {
                // 线程本身没有 offset 信息，但可以通过其他方式排序
                // 这里简单地按创建时间排序（后续可优化）
                return a.id.localeCompare(b.id);
            });
        }

        return map;
        // 依赖项：threads 的指纹列表（线程内消息内容变化不触发重建）
        // 使用 join(',') 作为简单哈希，避免对象引用比较
    }, [threads.map((t: any) => String(t.anchorFingerprint)).join(',')]);

    /**
     * 创建线程感知的组件集合
     *
     * 依赖项：threads 的指纹列表（同 threadByFingerprint）
     * 只有线程位置/数量变化时才重建组件，线程内消息变化不触发重建
     */
    const components = useMemo(() => {
        // 基础组件（无线程包装）
        const baseComponents = {
            /**
             * 自定义 <code> 标签渲染
             * 区分行内代码和代码块，分别应用不同样式
             *
             * react-markdown v9 移除了 `inline` prop，改为通过 className 判断：
             * - 行内代码（反引号）：无 className
             * - 代码块（围栏代码）：有 className（如 "language-python"）
             * - 代码块无语言：无 className，但被 <pre> 包裹（由 pre 组件处理）
             */
            code({ node, className, children, ...props }: any) {
                // 行内代码无 className，代码块有 language- 前缀的 className
                const isInline = !className;

                return isInline ? (
                    <code className="inline-code" {...props}>
                        {children}
                    </code>
                ) : (
                    // 代码块仅渲染 <code>，外层 wrapper 由 pre 组件负责
                    <code className={className} {...props}>
                        {children}
                    </code>
                );
            },
            /**
             * 自定义 <a> 标签渲染
             * 所有链接在新标签页打开，并添加安全属性
             */
            a({ node, children, href, ...props }: any) {
                return (
                    <a href={href} target="_blank" rel="noopener noreferrer" {...props}>
                        {children}
                    </a>
                );
            },
            /**
             * 自定义 <table> 标签渲染
             * 用可滚动的容器包裹表格，防止宽表格撑破布局
             * 支持追问线程：从 hast 节点中提取 tr 的指纹，匹配线程后渲染在表格下方
             */
            table({ node, children, ...props }: any) {
                // 从 hast 节点中提取所有 <tr> 的指纹
                const tableThreads = extractTableThreadMatches(node, threadByFingerprint, threadsRef);

                return (
                    <div className="table-wrapper">
                        <table {...props}>{children}</table>
                        {/* 表格追问线程：渲染在表格下方 */}
                        {tableThreads.length > 0 && (
                            tableThreads.length >= THREAD_GROUP_THRESHOLD ? (
                                <ThreadGroupEntry
                                    threads={tableThreads}
                                    onToggle={(threadId) => onThreadAction?.('toggle', threadId)}
                                    onDelete={(threadId) => onThreadAction?.('delete', threadId)}
                                    isStreaming={false}
                                    chatSearchKeyword={searchKeyword}
                                    isThreadStreaming={isThreadStreamingById}
                                    onActivate={onActivateThread}
                                    nestingLevel={nestingLevel}
                                    onSubThreadAction={onSubThreadAction}
                                    isSubThreadStreamingById={isSubThreadStreamingById}
                                    isSubThreadCollapsed={isSubThreadCollapsed}
                                />
                            ) : (
                                tableThreads.map((thread) => (
                                    <InlineThread
                                        key={thread.id}
                                        thread={thread}
                                        collapsed={isThreadCollapsed(thread.id)}
                                        onToggle={() => onThreadAction?.('toggle', thread.id)}
                                        onDelete={() => onThreadAction?.('delete', thread.id)}
                                        isStreaming={isThreadStreamingById(thread.id)}
                                        chatSearchKeyword={searchKeyword}
                                        onMessageDelete={(messageId: any, shouldPairDelete: any) => onThreadAction?.('messageDelete', thread.id, messageId, shouldPairDelete)}
                                        onMessageEdit={(messageId: any, newContent: any) => onThreadAction?.('messageEdit', thread.id, messageId, newContent)}
                                        onActivate={onActivateThread}
                                        nestingLevel={nestingLevel}
                                        onSubThreadAction={onSubThreadAction}
                                        isSubThreadStreamingById={isSubThreadStreamingById}
                                        isSubThreadCollapsed={isSubThreadCollapsed}
                                    />
                                ))
                            )
                        )}
                    </div>
                );
            },
            /**
             * 自定义 <pre> 标签渲染（代码块容器）
             *
             * react-markdown v9 中 <pre> 仅包裹代码块（围栏代码），
             * 行内代码不会经过此组件。因此将 code-block-wrapper 和语言标签
             * 移到此处，避免在行内代码上误加块级 wrapper 导致换行。
             */
            pre({ node, children, ...props }: any) {
                // 从子 <code> 元素的 className 提取语言
                const codeEl = node?.children?.find((c: any) => c.tagName === 'code');
                const codeCls = Array.isArray(codeEl?.properties?.className)
                    ? codeEl.properties.className.join(' ')
                    : (codeEl?.properties?.className || '');
                const match = /language-(\w+)/.exec(codeCls);
                const lang = match ? match[1] : '';

                return (
                    <div className="code-block-wrapper">
                        {lang && <div className="code-block-lang">{lang}</div>}
                        <pre {...props}>{children}</pre>
                    </div>
                );
            },
            /**
             * 自定义 <img> 标签渲染
             * Markdown 中的图片使用 ChatImageLightbox 组件包裹
             * 实现点击放大查看大图的功能
             */
            img({ node, src, alt, ...props }: any) {
                // 没有图片源则不渲染
                if (!src) return null;
                return (
                    <ChatImageLightbox
                        src={src}                                           // 图片 URL
                        alt={typeof alt === 'string' ? alt : ''}            // 替代文本
                        imgProps={props}                                    // 其他 img 属性
                    />
                );
            },
        };

        // 包装为线程感知组件，传入 threadsRef 以便 wrapper 组件读取最新线程数据
        return createThreadAwareComponents(baseComponents, threadByFingerprint, onThreadAction, isThreadStreamingById, searchKeyword, threadsRef, isThreadCollapsed, onActivateThread, nestingLevel, onSubThreadAction, isSubThreadStreamingById, isSubThreadCollapsed);
    }, [threads.map((t: any) => String(t.anchorFingerprint)).join(','), onThreadAction, isThreadStreamingById, searchKeyword, isThreadCollapsed, onActivateThread, nestingLevel, onSubThreadAction, isSubThreadStreamingById, isSubThreadCollapsed]);

    return (
        // 外层容器 - 使用 CSS 类名控制样式
        <div className="markdown-content">
            <ReactMarkdown
                // remark 插件配置（Markdown 语法层面的处理）
                remarkPlugins={remarkPlugins}
                // rehype 插件配置（HTML 输出层面的处理）
                rehypePlugins={rehypePlugins}
                // 自定义各 HTML 标签的渲染方式
                components={components}
            >
                {/* 要渲染的 Markdown 内容（已预处理 LaTeX 分隔符） */}
                {processedContent}
            </ReactMarkdown>
        </div>
    );
};

/**
 * 使用 React.memo 优化渲染性能
 *
 * Phase 6 优化：支持并行流式
 * - isThreadStreamingById 是函数，无法直接比较
 * - 当流式状态变化时，父组件会传入新的函数引用
 * - 因此移除函数比较，让组件在流式变化时正常重渲染
 *
 * 自定义比较函数：
 * - content 相同
 * - searchKeyword 相同
 * - threads 数组引用相同（或长度和指纹相同）
 * - onThreadAction 引用相同
 *
 * 只有以上条件变化时才重新渲染
 */
export default memo(MarkdownRenderer, (prevProps, nextProps) => {
    return (
        prevProps.content === nextProps.content &&
        prevProps.searchKeyword === nextProps.searchKeyword &&
        prevProps.threads === nextProps.threads &&
        prevProps.onThreadAction === nextProps.onThreadAction &&
        prevProps.isThreadCollapsed === nextProps.isThreadCollapsed &&
        prevProps.nestingLevel === nextProps.nestingLevel
        // Phase 6: 移除 isThreadStreamingById 比较（函数无法直接比较）
        // 流式状态变化时，父组件会更新，组件会正常重渲染
    );
});
