/**
 * useQuickTranslate — 选中文本快捷翻译 Hook
 *
 * 右键菜单"快捷翻译"选项的翻译逻辑：
 * - 调用翻译 API（translateText）直接翻译选中文本
 * - 结果显示在选区附近的浮层中
 * - 5 秒后自动隐藏，或点击空白处关闭
 */
import { useState, useCallback, useRef, useEffect } from 'react';
import { translateText } from '../../../services/translationApi';

export default function useQuickTranslate() {
  const [tooltip, setTooltip] = useState({
    visible: false,
    text: '',
    translation: '',
    x: 0,
    y: 0,
    loading: false,
  });

  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const abortControllerRef = useRef<AbortController | null>(null);

  const translate = useCallback(async (text: string, x: number, y: number) => {
    // Abort previous request
    abortControllerRef.current?.abort();

    // Create new abort controller for this request
    const controller = new AbortController();
    abortControllerRef.current = controller;

    // Clear previous auto-hide timer
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }

    setTooltip({
      visible: true,
      text,
      translation: '',
      x,
      y,
      loading: true,
    });

    try {
      const result = await translateText(text, { signal: controller.signal });

      // Don't update state if this request was aborted
      if (controller.signal.aborted) return;

      setTooltip((prev) => ({
        ...prev,
        translation: result.translation || '',
        loading: false,
      }));
      // Auto-hide after 5 seconds
      timerRef.current = setTimeout(() => {
        setTooltip((prev) => ({ ...prev, visible: false }));
      }, 5000);
    } catch (err) {
      // Don't show error if this was an aborted request
      if (controller.signal.aborted) return;

      setTooltip((prev) => ({
        ...prev,
        translation: `翻译失败: ${(err as Error).message || '未知错误'}`,
        loading: false,
      }));
    }
  }, []);

  const hide = useCallback(() => {
    // Abort any in-flight request
    abortControllerRef.current?.abort();
    abortControllerRef.current = null;

    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    setTooltip((prev) => ({ ...prev, visible: false }));
  }, []);

  // Cleanup timer and abort controller on unmount
  useEffect(() => {
    return () => {
      if (timerRef.current) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      abortControllerRef.current?.abort();
      abortControllerRef.current = null;
    };
  }, []);

  return { tooltip, translate, hide };
}
