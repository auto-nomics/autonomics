/**
 * remarkThreadAnchor.js
 * ============================================================
 * remark 插件：为 Markdown 块级元素注入源位置和指纹属性
 *
 * 本插件在 remark AST（mdast）层面遍历块级节点，为每个节点注入：
 * - data-source-start: 节点在原始 markdown 中的起始偏移量
 * - data-source-end: 节点在原始 markdown 中的结束偏移量
 * - data-block-fingerprint: 节点内容的哈希指纹，用于匹配线程
 *
 * 设计决策：
 * - 在 remark 阶段而非 rehype 阶段注入，因为 mdast position 100% 指向原始 markdown
 * - 通过 node.data.hProperties 桥接机制传递属性到 hast（remark-rehype 标准做法）
 * - 指纹包含位置信息（content + startOffset），确保相同文本在不同位置有不同指纹
 *
 * 注意：rehypeRaw 会重建 hast 树导致 hProperties 丢失。
 * 当传入 fingerprintStore 时，插件会把指纹映射存入 store，
 * 供后续 rehypeRestoreThreadAnchors 插件在 rehypeRaw 之后恢复属性。
 *
 * @module ai-chat/utils/remarkThreadAnchor
 */

import { visit } from 'unist-util-visit';
import { computeFingerprint } from './threadUtils';

/**
 * 支持的块级节点类型（mdast 节点类型）
 */
const BLOCK_TYPES = ['paragraph', 'listItem', 'heading', 'blockquote', 'tableRow'];

/**
 * 从 mdast 节点提取纯文本内容（用于作为指纹恢复的匹配 key）
 */
function extractMdastText(node: any) {
  let text = '';
  if (node.value) {
    text += node.value;
  }
  if (node.children) {
    for (const child of node.children) {
      text += extractMdastText(child);
    }
  }
  return text;
}

/**
 * remarkThreadAnchor 插件工厂函数
 *
 * @param {string} content - 原始 markdown 源文本
 * @param {Map} [fingerprintStore] - 可选，用于在 rehypeRaw 之后恢复属性的共享 Map
 * @returns {function} remark 插件函数
 */
export default function remarkThreadAnchor(content: any, fingerprintStore: any) {
  return function (tree: any) {
    visit(tree, (node) => {
      if (node.position && BLOCK_TYPES.includes(node.type)) {
        const start = node.position.start.offset;
        const end = node.position.end.offset;

        const rawSource = content ? content.slice(start, end) : '';

        const fingerprint = computeFingerprint(rawSource, start);

        // 注入 hProperties（remark-rehype 标准桥接）
        node.data = node.data || {};
        node.data.hProperties = {
          ...(node.data.hProperties || {}),
          'data-source-start': String(start),
          'data-source-end': String(end),
          'data-block-fingerprint': fingerprint,
        };

        // 如果提供了 fingerprintStore，保存映射供 rehypeRaw 之后恢复
        if (fingerprintStore) {
          const textKey = extractMdastText(node).trim();
          fingerprintStore.set(textKey, { start, end, fingerprint });
        }
      }
    });
  };
}

/**
 * rehype 插件：在 rehypeRaw 之后恢复被清除的 data-* 属性
 *
 * rehypeRaw 会重建 hast 树，导致 remarkThreadAnchor 通过 hProperties
 * 注入的 data-source-start / data-block-fingerprint 等属性丢失。
 * 本插件根据 remark 阶段保存的指纹映射（按文本内容匹配），重新注入这些属性。
 *
 * @param {Map} fingerprintStore - remarkThreadAnchor 保存的指纹映射
 * @returns {function} rehype 插件函数
 */
export function rehypeRestoreThreadAnchors(fingerprintStore: any) {
  return function (tree: any) {
    if (!fingerprintStore || fingerprintStore.size === 0) return;

    const BLOCK_TAGS = ['p', 'li', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'blockquote', 'tr'];

    visit(tree, 'element', (node) => {
      if (!BLOCK_TAGS.includes(node.tagName)) return;

      const hastText = extractHastText(node).trim();
      if (!hastText) return;

      // 精确匹配
      let saved = fingerprintStore.get(hastText);

      // 精确匹配失败时，尝试前缀匹配
      // 原因：mdast 的 listItem 文本可能不包含嵌套列表内容，
      // 但 hast 的 <li> 会包含所有嵌套文本，导致 mdast 文本 < hast 文本
      if (!saved) {
        let bestKey = null;
        let bestLen = 0;
        for (const [storeKey, storeVal] of fingerprintStore) {
          if (hastText.startsWith(storeKey) && storeKey.length > bestLen) {
            bestKey = storeKey;
            bestLen = storeKey.length;
            saved = storeVal;
          }
        }
      }

      if (saved) {
        node.properties = node.properties || {};
        node.properties['data-source-start'] = String(saved.start);
        node.properties['data-source-end'] = String(saved.end);
        node.properties['data-block-fingerprint'] = saved.fingerprint;
      }
    });
  };
}

/**
 * 从 hast 节点提取纯文本内容
 */
function extractHastText(node: any) {
  let text = '';
  if (node.type === 'text') {
    text += node.value || '';
  }
  if (node.children) {
    for (const child of node.children) {
      text += extractHastText(child);
    }
  }
  return text;
}

export { BLOCK_TYPES };
