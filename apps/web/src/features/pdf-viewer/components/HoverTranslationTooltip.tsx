/**
 * HoverTranslationTooltip — 段落级悬停翻译浮层组件
 *
 * 显示整段 PDF 文本的中文翻译：
 * - 以段落为单位显示翻译结果
 * - 高亮鼠标所在句子对应的译文
 * - 半透明背景、圆角、不大面积遮挡 PDF 内容
 * - 深色模式通过 CSS 变量自动适配
 * - 视口边界约束：上下翻转放置（空间大的一侧）+ 水平方向不超出屏幕
 * - 自动滚动：悬浮句子变化时，面板自动滚动到对应译文
 *
 * @module HoverTranslationTooltip
 */

import React, { useEffect, useRef, useState, type CSSProperties } from 'react';
import type { SentencePair } from '../types';

/**
 * 剥离 HTML 标签（兜底清洗）：LLM 偶尔会把 MinerU 原文里的 <sup>...</sup> 等
 * 标签原样保留进译文（如 <sup>引言</sup>）。这里在渲染前剥掉标签字符，
 * 避免把字面量 <sup>...</sup> 显示给用户。
 */
function stripHtmlTags(s: string): string {
  return s.replace(/<[^>]*>/g, '');
}

interface HoverTranslationTooltipProps {
  visible: boolean;
  text: string;
  sentences: SentencePair[];
  /** 当前鼠标悬浮的句子索引（< 0 表示无悬浮，不高亮任何句子） */
  hoveredSentenceIndex: number;
  /** 浮层水平定位坐标（视口坐标，像素） */
  x: number;
  /** 浮层垂直定位坐标（= 段落底边 + 间距，视口坐标，像素）；下方放置时的 top */
  y: number;
  /** 段落顶边（视口坐标，像素）。与 y 一起用于"上下哪侧空间大放哪侧"的翻转判断 */
  paraTop?: number;
  /** 浮层最大宽度（像素），默认 500 */
  maxWidth?: number;
  onMouseEnter?: () => void;
  onMouseLeave?: () => void;
}

function HoverTranslationTooltip({
  visible,
  text,
  sentences,
  hoveredSentenceIndex,
  x,
  y,
  paraTop,
  maxWidth = 500,
  onMouseEnter,
  onMouseLeave,
}: HoverTranslationTooltipProps) {
  const tooltipRef = useRef<HTMLDivElement>(null);
  // 各句子元素的 DOM 引用映射（索引 → DOM 元素），用于自动滚动
  const sentenceRefs = useRef<Record<number, HTMLSpanElement>>({});

  const [viewportSize, setViewportSize] = useState({
    width: window.innerWidth,
    height: window.innerHeight,
  });

  useEffect(() => {
    const handleResize = () => {
      setViewportSize({ width: window.innerWidth, height: window.innerHeight });
    };
    window.addEventListener('resize', handleResize);
    return () => window.removeEventListener('resize', handleResize);
  }, []);

  // 自动滚动效果：当鼠标悬浮的句子变化时，将浮层内容滚动到对应译文位置
  useEffect(() => {
    if (
      sentences?.length > 0 &&
      hoveredSentenceIndex >= 0 &&
      sentenceRefs.current[hoveredSentenceIndex]
    ) {
      sentenceRefs.current[hoveredSentenceIndex].scrollIntoView({ block: 'nearest' });
    }
  }, [hoveredSentenceIndex, sentences]);

  // 浮层隐藏时清空句子 DOM 引用
  useEffect(() => {
    if (!visible) sentenceRefs.current = {};
  }, [visible]);

  if (!visible || (!text && !sentences?.length)) return null;

  const viewportWidth = viewportSize.width;
  const viewportHeight = viewportSize.height;
  const margin = 8;
  // 浮层与段落边缘的间距（hook 传下的 y = paraBottom + 8，已含此间距）
  const gap = 8;

  // 垂直放置：比较段落下方/上方的剩余空间，大的一侧放置（flip）。
  // maxHeight 取所选侧的真实剩余空间——不再人为抬高最小高度，
  // 这是旧实现"面板戳出窗口底边"的根因（Math.max(60, 剩余空间) 会覆盖视口约束）。
  const spaceBelow = viewportHeight - margin - y;
  const spaceAbove = paraTop != null ? paraTop - margin - gap : -1;
  // paraTop 未传时不翻转（保持旧的下放行为），避免对 undefined 取坐标
  const placeAbove = paraTop != null && spaceAbove > spaceBelow;
  const chosenSpace = placeAbove ? spaceAbove : spaceBelow;
  // 32px 是防负值/极小值的兜底：仅当两侧空间都枯竭（视口比段落还矮）才会生效，
  // 此时溢出上限也就几个像素，远好于旧实现的无条件 60px 下探。
  const clampedMaxHeight = Math.max(32, Math.min(300, chosenSpace));

  // 水平视口边界约束：浮层居中于 x，左右各留 margin
  const halfWidth = Math.min(maxWidth, viewportWidth - margin * 2) / 2;
  let effectiveX = x;
  if (effectiveX - halfWidth < margin) effectiveX = halfWidth + margin;
  else if (effectiveX + halfWidth > viewportWidth - margin) effectiveX = viewportWidth - margin - halfWidth;

  const effectiveMaxWidth = Math.min(maxWidth, viewportWidth - margin * 2);

  // 半透明毛玻璃减轻遮挡感：92% 不透明度 + backdrop blur。
  // color-mix 不支持时整条声明失效退化为纯透明——所以保留 border/shadow 兜底可读性。
  // （WebKitGTK 2.38+ / Chrome 111+ / Firefox 113+ 均支持）
  const panelBackground = 'color-mix(in srgb, var(--bg-elevated) 92%, transparent)';

  const tooltipStyle: CSSProperties = {
    position: 'fixed',
    left: effectiveX,
    // 上方放置时 top 锚在段落顶边上方，再按自身高度整体上移（无需测量实际高度）
    top: placeAbove ? paraTop! - gap : y,
    transform: placeAbove ? 'translate(-50%, -100%)' : 'translateX(-50%)',
    maxWidth: effectiveMaxWidth,
    maxHeight: clampedMaxHeight,
    overflowY: 'auto',
    zIndex: 100,
    pointerEvents: 'auto',
    background: panelBackground,
    backdropFilter: 'blur(8px)',
    WebkitBackdropFilter: 'blur(8px)',
    border: '1px solid var(--border-color)',
    borderRadius: 6,
    padding: '8px 12px',
    fontSize: 12,
    lineHeight: 1.6,
    color: 'var(--text-primary)',
    boxShadow: 'var(--shadow-md)',
    wordBreak: 'break-word',
    whiteSpace: 'pre-wrap',
    transition: 'opacity 0.15s ease',
  };

  // 当前悬浮句子的高亮样式：左侧色条标记，避免大面积背景色干扰阅读
  const highlightStyle: CSSProperties = {
    background: 'var(--color-keypoint-bg)',
    borderLeft: '2px solid var(--color-keypoint)',
    paddingLeft: 6,
    marginLeft: -8,
    borderRadius: 2,
    display: 'block',
  };

  const hasSentences = sentences && sentences.length > 0;

  return (
    <div
      ref={tooltipRef}
      style={tooltipStyle}
      onMouseEnter={() => onMouseEnter?.()}
      onMouseLeave={() => onMouseLeave?.()}
      onWheel={(e) => e.stopPropagation()}
    >
      {hasSentences ? (
        <div>
          {sentences.map((s: SentencePair, idx: number) => (
            <span
              key={idx}
              ref={(el) => { if (el) sentenceRefs.current[idx] = el; }}
              style={idx === hoveredSentenceIndex ? highlightStyle : undefined}
            >
              {stripHtmlTags(s.translated)}{idx < sentences.length - 1 ? ' ' : ''}
            </span>
          ))}
        </div>
      ) : (
        stripHtmlTags(text)
      )}
    </div>
  );
}

export default HoverTranslationTooltip;
