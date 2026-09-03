/**
 * TermTooltip — 双击术语翻译浮层组件
 *
 * 显示 PDF 双击选中的英文术语的中文翻译。
 * 这是一个紧凑的单行浮层，适合短文本（单词/短语）的翻译显示。
 *
 * 功能特点：
 * - 紧凑的单行浮层，显示原文（灰色）和翻译（加粗）
 * - 支持流式显示：首 token 到达后立即显示，带闪烁光标
 * - 半透明背景、圆角、轻量阴影
 * - 深色模式通过 CSS 变量自动适配，无需 JS 检测
 * - Loading 状态显示（"term → ..."格式）
 * - 视口边界约束：水平和垂直方向不超出屏幕
 * - pointerEvents: none，不阻挡鼠标事件（因为是只读浮层）
 *
 * 与 HoverTranslationTooltip 的区别：
 * - TermTooltip：单行、紧凑、用于单词/短语的即时翻译
 * - HoverTranslationTooltip：多行、丰富、用于段落级的悬停翻译
 *
 * @module TermTooltip
 */

import React from 'react';
import PropTypes from 'prop-types';

/**
 * 双击术语翻译浮层组件
 *
 * 渲染逻辑根据状态分三种模式：
 * 1. 初始 Loading（loading && !translation）：显示 "term → ..." 格式
 * 2. 流式显示（loading && translation）：显示原文 + 部分翻译 + 闪烁光标
 * 3. 完成显示（!loading && translation）：显示原文 + 完整翻译
 *
 * @param {Object} props - 组件属性
 * @param {boolean} props.visible - 是否可见
 * @param {string} props.term - 原始术语（英文原文）
 * @param {string} props.translation - 翻译结果（流式时为部分翻译）
 * @param {number} props.x - X 坐标（视口相对，像素）
 * @param {number} props.y - Y 坐标（视口相对，像素）
 * @param {boolean} [props.loading=false] - 是否正在加载翻译
 * @returns {JSX.Element | null} 浮层 JSX 或 null
 */
function TermTooltip({
  visible,               // 是否可见
  term,                  // 原始术语
  translation,           // 翻译结果（流式时为部分翻译）
  x,                     // X 坐标
  y,                     // Y 坐标
  loading = false,       // 是否正在加载（默认 false）
}: {
  visible: boolean;
  term: string;
  translation: string;
  x: number;
  y: number;
  loading?: boolean;
}) {
  // 不可见或无内容时不渲染
  if (!visible || (!term && !loading)) {
    return null;
  }

  // ========================================
  // 尺寸和视口约束计算
  // ========================================

  const maxWidth = 280;   // 浮层最大宽度（像素）
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
   * 估算浮层高度约 60px。
   */
  const effectiveY = Math.min(y + 12, vh - 60);

  // 是否处于流式显示模式：loading 为 true 且已有部分翻译文本
  const isStreaming = loading && translation;

  /**
   * 原始术语的灰色文字样式
   *
   * 原始术语以较小的灰色字体显示在翻译之前，作为上下文提示。
   * 所有颜色使用 CSS 变量，深色/浅色模式自动切换。
   */
  const termMutedStyle = {
    color: 'var(--text-tertiary)', // 次要文字颜色（灰色）
    marginRight: 4,                // 与翻译结果的间距 4px
    fontSize: 11,                  // 11px 字体（比翻译小 1px）
  };

  return (
    <div
      style={{
          position: 'fixed',                        // 固定定位，基于视口坐标
          left: effectiveX,                         // 水平位置（已约束）
          top: Math.max(margin, effectiveY),        // 垂直位置（已约束，最小为 margin）
          transform: 'translateX(-50%)',            // 水平居中偏移
          zIndex: 110,                              // 层级 110（高于 HoverTranslationTooltip 的 100）
          pointerEvents: 'none',                    // 不阻挡鼠标事件（只读浮层）
          background: 'var(--bg-elevated)',         // 背景色（CSS 变量）
          border: '1px solid var(--border-color)',  // 边框（CSS 变量）
          borderRadius: 4,                          // 4px 圆角（比 HoverTranslationTooltip 更小）
          padding: '3px 8px',                       // 更紧凑的内边距
          fontSize: 12,                             // 12px 字体
          lineHeight: 1.4,                          // 1.4 倍行高
          color: 'var(--text-primary)',             // 主文字颜色（CSS 变量）
          boxShadow: 'var(--shadow-md)',            // 中等阴影（CSS 变量）
          whiteSpace: 'nowrap',                     // 不换行（保持单行显示）
          maxWidth: 280,                            // 最大宽度 280px
          overflow: 'hidden',                       // 隐藏溢出内容
          textOverflow: 'ellipsis',                 // 文本溢出显示省略号
          transition: 'opacity 0.15s ease',         // 透明度过渡动画
          opacity: loading && !translation ? 0.8 : 1, // 初始 loading 时半透明
        }}
    >
      {/* 状态1：初始 Loading — 无翻译结果，显示 "term → ..." */}
      {loading && !translation && (
        <span style={{ opacity: 0.6, fontSize: 11 }}>
          {term} → ...
        </span>
      )}
      {/* 状态2：流式显示 — 有部分翻译，显示原文 + 翻译 + 闪烁光标 */}
      {isStreaming && (
        <span>
          <span style={termMutedStyle}>{term}</span>
          <span style={{ fontWeight: 500 }}>{translation}</span>
          {/* 闪烁光标：模拟打字效果 */}
          <span style={{
            display: 'inline-block',                  // 行内块元素
            width: 1,                                 // 1px 宽度（比 HoverTranslationTooltip 更细）
            height: 12,                               // 12px 高度
            background: 'var(--text-tertiary)',       // 次要文字颜色（CSS 变量）
            marginLeft: 1,                            // 与文字间距 1px
            verticalAlign: 'text-bottom',             // 底部对齐
            animation: 'hover-tooltip-blink-cursor 0.6s step-end infinite', // 闪烁动画
          }} />
        </span>
      )}
      {/* 状态3：完成 — 显示完整翻译 */}
      {!loading && translation && (
        <span>
          <span style={termMutedStyle}>{term}</span>
          <span style={{ fontWeight: 500 }}>
            {translation}
          </span>
        </span>
      )}
    </div>
    );
}

/**
 * PropTypes 类型检查（运行时）
 *
 * 为组件添加运行时 props 类型检查，便于开发时调试。
 */
TermTooltip.propTypes = {
  visible: PropTypes.bool,       // 是否可见
  term: PropTypes.string,        // 原始术语
  translation: PropTypes.string, // 翻译结果
  x: PropTypes.number,           // X 坐标
  y: PropTypes.number,           // Y 坐标
  loading: PropTypes.bool,       // 是否正在加载
};

export default TermTooltip;
