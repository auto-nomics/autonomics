/**
 * useSelectedPaperDetail - 选中文献的详情获取 + 翻译状态轮询
 *
 * 监听 selectedPaper 变化，调 `getPaper(id)` 拉取完整双语快照（含 abstract、
 * title_zh、abstract_zh、paragraphTranslationStatus）。若后端正在生成翻译
 * （status === 'generating'），每 1.5s 轮询一次直到 done/failed 或 30s 超时。
 *
 * 替代旧 `useTranslationCache`：原 hook 同时消费 list item（abstract/title_zh
 * 都是 None）和 translate API（只返中文），导致 ScreeningPaperDetail 上下半
 * 面板数据源不对称，出现「上无摘要 / 下有中文翻译」的错位。统一改用 detail
 * API 单源后该类错位不可能再发生。
 */
import { useState, useEffect } from 'react';
import type { Paper } from '@/types';
import { getPaper } from '../../../services/papersApi';

const POLL_INTERVAL_MS = 1500;
const POLL_TIMEOUT_MS = 30000;

export function useSelectedPaperDetail(selectedPaper: Paper | null) {
  // 乐观初始化：选中瞬间立即用 list item 作为初值，标题等基础字段立即可见
  const [detail, setDetail] = useState<Paper | null>(selectedPaper);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!selectedPaper) {
      setDetail(null);
      setLoading(false);
      return;
    }

    // 乐观渲染：list item 至少带 title，避免标题区短暂空白
    setDetail(selectedPaper);
    setLoading(true);

    const controller = new AbortController();
    let pollInterval: ReturnType<typeof setInterval> | null = null;
    let pollTimeout: ReturnType<typeof setTimeout> | null = null;

    const stopPolling = () => {
      if (pollInterval) {
        clearInterval(pollInterval);
        pollInterval = null;
      }
      if (pollTimeout) {
        clearTimeout(pollTimeout);
        pollTimeout = null;
      }
    };

    const pull = async () => {
      try {
        const result = await getPaper(selectedPaper.id, controller.signal);
        setDetail(result);
        setLoading(false);

        // 翻译生成中：开轮询直到状态稳定或超时
        if (result.paragraphTranslationStatus === 'generating') {
          pollInterval = setInterval(async () => {
            try {
              const next = await getPaper(selectedPaper.id, controller.signal);
              setDetail(next);
              if (next.paragraphTranslationStatus !== 'generating') {
                stopPolling();
              }
            } catch {
              // 单次轮询失败容忍，继续重试直到超时兜底
            }
          }, POLL_INTERVAL_MS);

          pollTimeout = setTimeout(stopPolling, POLL_TIMEOUT_MS);
        }
      } catch (err: any) {
        setLoading(false);
        if (err?.name !== 'AbortError') {
          console.error('获取文献详情失败:', err);
        }
      }
    };

    pull();

    return () => {
      controller.abort();
      stopPolling();
    };
  }, [selectedPaper]);

  return { detail, loading };
}
