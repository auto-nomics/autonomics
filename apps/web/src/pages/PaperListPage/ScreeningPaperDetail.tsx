/**
 * Autonomics 筛选模式论文详情面板组件
 *
 * 在筛选模式（Screening Mode）中，当用户点击某篇文献行时，
 * 左面板从分类树视图切换到此组件，分为上下两个子面板：
 * - 上半部分：显示论文标题和摘要原文
 * - 下半部分：显示标题和摘要的中文翻译（由后端 AI 翻译并缓存）
 *
 * 功能特性：
 * - 同步滚动：上下面板滚动联动，百分比同步 + 悬浮时锚点居中
 * - 句子级高亮：鼠标悬浮某句，原文+译文对应句同时高亮
 *   - 前端 splitSentences + index-based 匹配（旧 abstract_sentences 后端对齐
 *     分支已删除——后端 TranslateStatusResponse 从未真正返回过该字段，是死代码）
 *
 * Props：
 * @param {Paper|null} detail - 文献完整详情（来自 GET /api/papers/{id}，
 *   含 abstract / title_zh / abstract_zh / paragraphTranslationStatus）
 * @param {boolean} loading - 详情是否仍在加载（含翻译生成轮询阶段）
 */
import React, { useState, useRef, useCallback, useEffect, useMemo } from 'react';
import type { Paper } from '@/types';
import { useTranslation } from 'react-i18next';
import { Spin } from 'antd';
import { TranslationOutlined } from '@ant-design/icons';

// ========== 常见学术缩写白名单 ==========
// 这些词后的 '.' 不应被当作句子结束符
const ABBREVIATIONS = new Set([
  'dr', 'mr', 'mrs', 'ms', 'prof',
  'e.g', 'i.e', 'etc', 'vs', 'cf', 'al',
  'fig', 'eq', 'no',
]);

/**
 * 将文本按句子分割
 *
 * 支持中英文混合文本：
 * - 英文：.!? 后跟空格+大写字母或文本结尾 → 断句（跳过缩写白名单）
 * - 中文：。！？ → 直接断句
 *
 * 已知限制：无法覆盖所有缩写情况，准确率约 95%
 *
 * @param {string} text - 待分割的文本
 * @returns {string[]} 句子数组
 */
function splitSentences(text: any) {
  if (!text || typeof text !== 'string') return [];
  const trimmed = text.trim();
  if (!trimmed) return [];

  const sentences = [];
  let current = '';

  for (let i = 0; i < trimmed.length; i++) {
    const ch = trimmed[i];
    current += ch;

    // 中文句末标点：直接断句
    if ('。！？'.includes(ch)) {
      // 消耗后续空格
      while (i + 1 < trimmed.length && trimmed[i + 1] === ' ') {
        i++;
        current += trimmed[i];
      }
      sentences.push(current.trim());
      current = '';
    }
    // 英文句末标点：需判断是否为真正的句子结束
    else if ('.!?'.includes(ch)) {
      // 检查缩写白名单：取 current 最后一个"词"判断
      const lastWord = current.trim().split(/\s+/).pop()?.replace(/[.!?]+$/, '').toLowerCase();
      if (lastWord && ABBREVIATIONS.has(lastWord)) {
        continue; // 缩写，不断句
      }

      const rest = trimmed.slice(i + 1);
      // 后面是空格+大写字母，或文本结尾 → 断句
      if (rest === '' || /^\s+[A-Z]/.test(rest)) {
        while (i + 1 < trimmed.length && trimmed[i + 1] === ' ') {
          i++;
          current += trimmed[i];
        }
        sentences.push(current.trim());
        current = '';
      }
    }
  }

  // 处理不以标点结尾的剩余文本
  if (current.trim()) {
    sentences.push(current.trim());
  }

  return sentences.filter(s => s.length > 0);
}

/**
 * 筛选模式论文详情面板
 *
 * 布局结构：
 * +---------------------------+
 * | 原文标题 (可高亮)         |
 * | 原文句子1 | 句子2 | ...   |
 * +---------------------------+  ← 分割线
 * | 译文标题 (可高亮)         |
 * | 译文句子1 | 句子2 | ...   |
 * +---------------------------+
 */
export default function ScreeningPaperDetail({
  detail,
  loading,
}: {
  detail: Paper | null;
  loading: boolean;
}) {
  const { t } = useTranslation('paperList');
  // ========== 状态 ==========
  // hoveredIdx: null=无悬浮, -1=标题, 0/1/2...=句子索引
  const [hoveredIdx, setHoveredIdx] = useState<number | null>(null);

  // ========== DOM 引用 ==========
  const originalRef = useRef<HTMLDivElement>(null);       // 原文面板
  const translationRef = useRef<HTMLDivElement>(null);    // 译文面板
  const isSyncingRef = useRef(false);     // 滚动同步防循环标志

  // ========== 句子数据计算 ==========

  /**
   * 前端 splitSentences 分句 + index-based 匹配
   *
   * 返回格式统一为 { en: string[], zh: string[] }
   */
  const sentences = useMemo(() => {
    const en = splitSentences(detail?.abstract);
    const zh = splitSentences(detail?.abstract_zh);
    return { en, zh };
  }, [detail?.abstract, detail?.abstract_zh]);

  // ========== 同步滚动 ==========

  /**
   * 百分比同步滚动
   * 使用 isSyncingRef + setTimeout(50ms) 防止循环触发
   *
   * 注意：不能使用 requestAnimationFrame，因为程序化 scrollTop 变化产生的
   * scroll 事件可能在 rAF 回调之后才到达，导致 guard 被提前重置，
   * 触发反向同步覆盖用户滚动。
   */
  const handleScroll = useCallback((source: any) => {
    if (isSyncingRef.current) return;

    const sourceEl = source === 'original' ? originalRef.current : translationRef.current;
    const targetEl = source === 'original' ? translationRef.current : originalRef.current;

    if (!sourceEl || !targetEl) return;

    isSyncingRef.current = true;
    const scrollRatio = sourceEl.scrollTop / (sourceEl.scrollHeight - sourceEl.clientHeight || 1);
    targetEl.scrollTop = scrollRatio * (targetEl.scrollHeight - targetEl.clientHeight);

    setTimeout(() => {
      isSyncingRef.current = false;
    }, 50);
  }, []);

  /**
   * 悬浮时锚点滚动：将两个面板都滚动到对应句子居中显示
   */
  useEffect(() => {
    if (hoveredIdx === null) return;

    // 标题不需要锚点滚动（标题在顶部，始终可见）
    if (hoveredIdx === -1) return;

    // 在两个面板中查找对应句子并滚动到居中
    const offsets = [originalRef, translationRef].map(ref => {
      const container = ref.current;
      if (!container) return null;
      const sentenceEl = container.querySelector(`[data-idx="${hoveredIdx}"]`);
      if (!sentenceEl) return null;

      const containerRect = container.getBoundingClientRect();
      const sentenceRect = sentenceEl.getBoundingClientRect();
      return {
        container,
        offset: sentenceRect.top - containerRect.top - containerRect.height / 2 + sentenceRect.height / 2,
      };
    });

    // 一次性设置 guard，批量滚动两个面板
    const validOffsets = offsets.filter(Boolean);
    if (validOffsets.length === 0) return;

    isSyncingRef.current = true;
    validOffsets.forEach((item: any) => {
      item.container.scrollTop += item.offset;
    });
    setTimeout(() => {
      isSyncingRef.current = false;
    }, 50);
  }, [hoveredIdx]);

  // ========== 事件委托处理 ==========

  const handleMouseOver = useCallback((e: any) => {
    const target = e.target.closest('[data-idx]');
    if (target) {
      setHoveredIdx(Number(target.dataset.idx));
    }
  }, []);

  const handleMouseOut = useCallback((e: any) => {
    // 只在离开面板时清除悬浮状态
    const panel = e.currentTarget;
    const related = e.relatedTarget;
    if (!related || !panel.contains(related)) {
      setHoveredIdx(null);
    }
  }, []);

  // ========== 边界情况处理 ==========
  if (!detail) {
    return (
      <div style={{
        display: 'flex',
        justifyContent: 'center',
        alignItems: 'center',
        height: '100%',
        color: 'var(--text-tertiary)',
        fontSize: 14,
        padding: 20,
        textAlign: 'center',
      }}>
        <div>
          <div style={{ marginBottom: 8, opacity: 0.5 }}>点击文献行查看详情</div>
          <div style={{ fontSize: 12, opacity: 0.4 }}>点击标题表头返回分类树</div>
        </div>
      </div>
    );
  }

  // ========== 高亮样式判断 ==========
  const isHighlighted = (idx: any) => hoveredIdx === idx;

  // ========== 渲染 ==========
  return (
    <div style={{
      display: 'flex',
      flexDirection: 'column',
      height: '100%',
      overflow: 'hidden',
    }}>
      {/* ========== 上半部分：原文标题和摘要 ========== */}
      <div
        ref={originalRef}
        onScroll={() => handleScroll('original')}
        onMouseOver={handleMouseOver}
        onMouseOut={handleMouseOut}
        style={{
          flex: 1,
          overflow: 'auto',
          padding: '12px 16px',
          borderBottom: '1px solid var(--border-color)',
        }}
      >
        {/* 原文标题 */}
        <div
          data-idx="-1"
          style={{
            fontWeight: 600,
            fontSize: 15,
            lineHeight: 1.5,
            marginBottom: 10,
            color: 'var(--text-primary)',
            backgroundColor: isHighlighted(-1) ? 'var(--bg-highlight)' : 'transparent',
            borderRadius: 2,
            transition: 'background-color 0.15s ease',
            cursor: 'default',
          }}
        >
          {detail.title || t('screening.unnamed')}
        </div>

        {/* 原文摘要（分句渲染） */}
        {detail.abstract ? (
          <div style={{ fontSize: 13, lineHeight: 1.7, color: 'var(--text-secondary)' }}>
            {sentences.en.map((s: any, i: any) => (
              <span
                key={i}
                data-idx={i}
                style={{
                  backgroundColor: isHighlighted(i) ? 'var(--bg-highlight)' : 'transparent',
                  transition: 'background-color 0.15s ease',
                  cursor: 'default',
                  borderRadius: 2,
                }}
              >
                {s}
              </span>
            ))}
          </div>
        ) : (
          <div style={{ fontSize: 13, color: 'var(--text-tertiary)', fontStyle: 'italic' }}>
            无摘要
          </div>
        )}
      </div>

      {/* ========== 下半部分：中文翻译 ========== */}
      <div
        ref={translationRef}
        onScroll={() => handleScroll('translation')}
        onMouseOver={handleMouseOver}
        onMouseOut={handleMouseOut}
        style={{
          flex: 1,
          overflow: 'auto',
          padding: '12px 16px',
        }}
      >
        {loading ? (
          <div style={{
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            justifyContent: 'center',
            padding: '24px 0',
            gap: 8,
          }}>
            <Spin size="small" />
            <span style={{ fontSize: 12, color: 'var(--text-tertiary)' }}>{t('screening.translating')}</span>
          </div>
        ) : detail?.title_zh ? (
          <>
            {/* 翻译后的标题 */}
            <div
              data-idx="-1"
              style={{
                fontWeight: 600,
                fontSize: 15,
                lineHeight: 1.5,
                marginBottom: 10,
                color: 'var(--text-primary)',
                backgroundColor: isHighlighted(-1) ? 'var(--bg-highlight)' : 'transparent',
                borderRadius: 2,
                transition: 'background-color 0.15s ease',
                cursor: 'default',
              }}
            >
              {detail.title_zh}
            </div>

            {/* 翻译后的摘要（分句渲染） */}
            {sentences.zh.length > 0 ? (
              <div style={{ fontSize: 13, lineHeight: 1.7, color: 'var(--text-secondary)' }}>
                {sentences.zh.map((s: any, i: any) => (
                  <span
                    key={i}
                    data-idx={i}
                    style={{
                      backgroundColor: isHighlighted(i) ? 'var(--bg-highlight)' : 'transparent',
                      transition: 'background-color 0.15s ease',
                      cursor: 'default',
                      borderRadius: 2,
                    }}
                  >
                    {s}
                  </span>
                ))}
              </div>
            ) : (
              <div style={{ fontSize: 13, color: 'var(--text-tertiary)', fontStyle: 'italic' }}>
                无摘要
              </div>
            )}
          </>
        ) : (
          <div style={{
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            justifyContent: 'center',
            padding: '24px 0',
            gap: 8,
          }}>
            <TranslationOutlined style={{ fontSize: 24, color: 'var(--border-color)' }} />
            <span style={{ fontSize: 12, color: 'var(--text-tertiary)' }}>{t('screening.translationUnavailable')}</span>
          </div>
        )}
      </div>
    </div>
  );
}
