/**
 * QuickTranslateTooltip — 选中文本快捷翻译浮层组件
 *
 * 显示右键"快捷翻译"的结果，适用于较长的文本选择翻译。
 *
 * 功能特点：
 * - 支持多行文本显示（与 TermTooltip 的单行限制不同）
 * - Loading 状态显示（"text → ..."格式）
 * - 流式显示：翻译过程中实时更新
 * - 完成后显示：原文（灰色小字）+ 翻译（加粗）
 * - 半透明背景、圆角、轻量阴影
 * - 深色模式通过 CSS 变量自动适配
 * - 视口边界约束：水平和垂直方向不超出屏幕
 * - 内容超出时可滚动（maxHeight + overflowY: auto）
 *
 * 与其他翻译浮层的区别：
 * - TermTooltip：单行、紧凑、用于单词/短语的即时翻译（双击触发）
 * - HoverTranslationTooltip：多行、句子级高亮、用于段落翻译（悬停触发）
 * - QuickTranslateTooltip：多行、简洁、用于选中文本的快捷翻译（右键菜单触发）
 *
 * @module QuickTranslateTooltip
 */
import React from 'react';
import PropTypes from 'prop-types';

/**
 * 剥离 HTML 标签（兜底清洗）：防止 LLM 偶发把标签字符保留进流式译文。
 */
function stripHtmlTags(s: string): string {
  return s.replace(/<[^>]*>/g, '');
}

/**
 * 快捷翻译浮层组件
 *
 * 渲染逻辑根据状态分三种模式：
 * 1. 初始 Loading（loading && !translation）：显示截断的原文 + "→ ..."
 * 2. 流式显示（loading && translation）：显示原文（灰色小字）+ 部分翻译（加粗）
 * 3. 完成显示（!loading && translation）：显示原文（灰色小字）+ 完整翻译（加粗）
 *
 * 注意：状态2和状态3的渲染结构相同，只是 loading 状态不同。
 * 之所以分开写而不是合并，是为了代码的可读性和后续可能的差异化处理。
 *
 * @param {Object} props - 组件属性
 * @param {boolean} props.visible - 是否可见
 * @param {string} props.text - 原始选中的文本
 * @param {string} props.translation - 翻译结果（流式时为部分翻译）
 * @param {number} props.x - X 坐标（视口相对，像素）
 * @param {number} props.y - Y 坐标（视口相对，像素）
 * @param {boolean} [props.loading=false] - 是否正在加载翻译
 * @returns {JSX.Element | null} 浮层 JSX 或 null
 */
function QuickTranslateTooltip({
  visible,               // 是否可见
  text,                  // 原始选中文本
  translation,           // 翻译结果
  x,                     // X 坐标
  y,                     // Y 坐标
  loading = false,       // 是否正在加载（默认 false）
}: {
  visible: boolean;
  text: string;
  translation: string;
  x: number;
  y: number;
  loading?: boolean;
}) {
  // 浮层不可见时不渲染
  if (!visible) return null;

  /**
   * 截断原文显示
   *
   * 如果原文超过 60 字符，截断并添加省略号。
   * 因为浮层空间有限，不需要显示完整的原文，只需要让用户知道翻译的是哪段文本。
   */
  const displayText = text.length > 60 ? text.slice(0, 60) + '...' : text;

  // ========================================
  // 尺寸和视口约束计算
  // ========================================

  const maxWidth = 400;   // 浮层最大宽度（像素），比 TermTooltip 大（400 vs 280）
  const maxHeight = 200;  // 浮层最大高度（像素），支持多行翻译
  const margin = 8;       // 浮层与视口边缘的最小间距（像素）
  const halfWidth = maxWidth / 2; // 最大宽度的一半，用于居中定位
  const vw = window.innerWidth;   // 视口宽度
  const vh = window.innerHeight;  // 视口高度

  /**
   * 水平视口边界约束
   *
   * 确保浮层水平方向不超出视口左右边缘。
   * 浮层使用 translateX(-50%) 居中，所以 effectiveX 是浮层中心位置。
   */
  let effectiveX = x;
  if (effectiveX - halfWidth < margin) {
    // 左边界约束：浮层左边缘不超出视口左边缘
    effectiveX = halfWidth + margin;
  } else if (effectiveX + halfWidth > vw - margin) {
    // 右边界约束：浮层右边缘不超出视口右边缘
    effectiveX = vw - margin - halfWidth;
  }

  /**
   * 垂直视口边界约束
   *
   * 浮层默认显示在 y + 12px 的位置（向下偏移 12px 避免遮挡文本）。
   * 约束确保浮层底部不超出视口底部。
   * 考虑了 maxHeight（200px）+ margin（8px）的空间。
   */
  const effectiveY = Math.min(y + 12, vh - maxHeight - margin);

  return (
    <div
      style={{
        position: 'fixed',                        // 固定定位，基于视口坐标
        left: effectiveX,                         // 水平位置（已约束）
        top: Math.max(margin, effectiveY),        // 垂直位置（已约束，最小为 margin）
        transform: 'translateX(-50%)',            // 水平居中偏移
        zIndex: 110,                              // 层级 110（与其他 tooltip 一致）
        background: 'var(--bg-elevated)',         // 背景色（CSS 变量，适配深色模式）
        border: '1px solid var(--border-color)',  // 边框（CSS 变量）
        borderRadius: 6,                          // 6px 圆角
        padding: '8px 12px',                      // 内边距
        fontSize: 13,                             // 13px 字体（比 TermTooltip 大 1px）
        lineHeight: 1.5,                          // 1.5 倍行高（更舒适的阅读体验）
        color: 'var(--text-primary)',             // 主文字颜色（CSS 变量）
        boxShadow: 'var(--shadow-md)',            // 中等阴影（CSS 变量）
        maxWidth: 400,                            // 最大宽度 400px
        maxHeight: 200,                           // 最大高度 200px
        overflowY: 'auto',                        // 内容超出时垂直滚动
        wordBreak: 'break-word',                  // 长单词换行
        transition: 'opacity 0.15s ease',         // 透明度过渡动画
        opacity: loading && !translation ? 0.8 : 1, // 初始 loading 时半透明
      }}
    >
      {/* 状态1：初始 Loading — 无翻译结果，显示截断原文 + "→ ..." */}
      {loading && !translation && (
        <span style={{ opacity: 0.6 }}>
          {displayText} → ...
        </span>
      )}
      {/* 状态2：流式显示 — 有部分翻译，显示原文（灰色小字）+ 部分翻译（加粗） */}
      {loading && translation && (
        <div>
          {/* 原文：以较小的灰色字体显示在上方，作为上下文提示 */}
          <div style={{ color: 'var(--text-tertiary)', fontSize: 11, marginBottom: 4 }}>
            {displayText}
          </div>
          {/* 翻译结果：加粗显示 */}
          <div style={{ fontWeight: 500 }}>{stripHtmlTags(translation)}</div>
        </div>
      )}
      {/* 状态3：完成 — 显示完整翻译结果（结构与流式显示相同） */}
      {!loading && translation && (
        <div>
          {/* 原文：以较小的灰色字体显示在上方 */}
          <div style={{ color: 'var(--text-tertiary)', fontSize: 11, marginBottom: 4 }}>
            {displayText}
          </div>
          {/* 完整翻译结果：加粗显示 */}
          <div style={{ fontWeight: 500 }}>{stripHtmlTags(translation)}</div>
        </div>
      )}
    </div>
  );
}

/**
 * PropTypes 类型检查（运行时）
 *
 * 为组件添加运行时 props 类型检查，便于开发时调试。
 */
QuickTranslateTooltip.propTypes = {
  visible: PropTypes.bool,       // 是否可见
  text: PropTypes.string,        // 原始选中文本
  translation: PropTypes.string, // 翻译结果
  x: PropTypes.number,           // X 坐标
  y: PropTypes.number,           // Y 坐标
  loading: PropTypes.bool,       // 是否正在加载
};

export default QuickTranslateTooltip;
