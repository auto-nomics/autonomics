/**
 * useDoubleClickTerm — PDF 双击术语翻译
 *
 * 实现双击 PDF 中的英文术语/短语时显示中文含义的功能：
 * - 监听 PDF 文本层的 dblclick 事件
 * - 获取双击位置的选中文本
 * - 调用流式翻译 API，逐 token 显示翻译结果
 * - 3秒后自动消失，或点击其他位置消失
 *
 * 设计理念（参考 GUIDE_READING_EXPERIENCE_REDESIGN.md 3.3.5）：
 * - 双击是读者遇到不理解术语时的自然反应
 * - 翻译是按需触发的，不会干扰正常阅读
 * - 轻量提示，不替代读者的独立理解过程
 * - 流式显示：首 token ~200ms 到达，渐进显示完整翻译
 *
 * @module useDoubleClickTerm
 */

import { useState, useCallback, useRef, useEffect } from 'react';
import { translateTextStream } from '../../../services/translationApi';

/**
 * 术语翻译缓存（全局，与悬停翻译共享概念）
 */
const termCache = new Map();
const TERM_CACHE_MAX = 200;

/**
 * 判断文本是否可能是需要翻译的英文术语
 *
 * 过滤规则：
 * - 纯数字/符号 → 不翻译
 * - 过短（< 2 字符）→ 不翻译
 * - 过长（> 80 字符）→ 不翻译（双击通常只选中一个词/短语）
 * - 已是中文 → 不翻译
 */
function isLikelyEnglishTerm(text: string): boolean {
  if (!text || text.length < 2 || text.length > 80) return false;
  if (/^[\d\s\p{P}]+$/u.test(text)) return false;
  if (/[\u4e00-\u9fff]/.test(text)) return false;
  return true;
}

/**
 * 双击术语翻译 Hook
 *
 * @param {object} params
 * @param {React.RefObject} params.pdfAreaRef - PDF 主渲染区域 DOM ref
 * @param {number} params.pageNumber - 当前页码（1-based）
 */
export function useDoubleClickTerm({
  pdfAreaRef,
  pageNumber,
}: {
  pdfAreaRef: React.RefObject<HTMLElement>;
  pageNumber: number;
}) {
  /**
   * 术语提示状态
   */
  const [termTooltip, setTermTooltip] = useState({
    visible: false,
    term: '',
    translation: '',
    x: 0,
    y: 0,
    loading: false,
  });

  const hideTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // 当前流式请求的 abort 控制器
  const streamAbortRef = useRef<{ abort: () => void } | null>(null);

  /**
   * 清除隐藏定时器
   */
  const clearHideTimer = useCallback(() => {
    if (hideTimerRef.current) {
      clearTimeout(hideTimerRef.current);
      hideTimerRef.current = null;
    }
  }, []);

  /**
   * 取消正在进行的流式请求
   */
  const abortStream = useCallback(() => {
    if (streamAbortRef.current) {
      streamAbortRef.current.abort();
      streamAbortRef.current = null;
    }
  }, []);

  /**
   * 隐藏提示
   */
  const hideTooltip = useCallback(() => {
    clearHideTimer();
    abortStream();
    setTermTooltip(prev => ({ ...prev, visible: false }));
  }, [clearHideTimer, abortStream]);

  /**
   * 处理双击事件
   */
  const handleDoubleClick = useCallback(async (event: React.MouseEvent) => {
    // 获取当前选区
    const selection = window.getSelection();
    const selectedText = selection?.toString().trim();

    if (!selectedText || !isLikelyEnglishTerm(selectedText)) {
      return;
    }

    // 获取点击位置（视口相对坐标，配合 position: fixed 定位）
    const x = event.clientX;
    const y = event.clientY;

    // 检查缓存
    const cacheKey = selectedText.toLowerCase().trim();
    if (termCache.has(cacheKey)) {
      clearHideTimer();
      abortStream();
      setTermTooltip({
        visible: true,
        term: selectedText,
        translation: termCache.get(cacheKey),
        x,
        y,
        loading: false,
      });
      // 3秒后自动隐藏
      hideTimerRef.current = setTimeout(() => {
        setTermTooltip(prev => ({ ...prev, visible: false }));
      }, 3000);
      return;
    }

    // 取消之前的流式请求
    abortStream();
    clearHideTimer();

    // 显示 loading 状态
    setTermTooltip({
      visible: true,
      term: selectedText,
      translation: '',
      x,
      y,
      loading: true,
    });

    // 使用流式翻译 API
    let accumulated = '';
    streamAbortRef.current = translateTextStream(selectedText, {
      onToken: (token) => {
        accumulated += token;
        // 首次 token 到达即取消 loading，显示流式文本
        setTermTooltip(prev => ({
          ...prev,
          translation: accumulated,
          loading: false,
        }));
      },
      onDone: (result) => {
        const translated = result.translation || accumulated;
        // 更新缓存
        if (termCache.size >= TERM_CACHE_MAX) {
          const firstKey = termCache.keys().next().value;
          termCache.delete(firstKey);
        }
        termCache.set(cacheKey, translated);

        setTermTooltip(prev => ({
          ...prev,
          translation: translated,
          loading: false,
        }));

        streamAbortRef.current = null;

        // 3秒后自动隐藏
        hideTimerRef.current = setTimeout(() => {
          setTermTooltip(prev => ({ ...prev, visible: false }));
        }, 3000);
      },
      onError: (error) => {
        console.error('流式术语翻译失败:', error);
        hideTooltip();
      },
    });
  }, [pdfAreaRef, clearHideTimer, abortStream, hideTooltip]);

  /**
   * 组件卸载时清理
   */
  useEffect(() => {
    return () => {
      clearHideTimer();
      abortStream();
    };
  }, [clearHideTimer, abortStream]);

  /**
   * 页码变化时隐藏
   */
  useEffect(() => {
    hideTooltip();
  }, [pageNumber, hideTooltip]);

  return {
    termTooltip,
    handleDoubleClick,
    hideTermTooltip: hideTooltip,
  };
}

export default useDoubleClickTerm;
