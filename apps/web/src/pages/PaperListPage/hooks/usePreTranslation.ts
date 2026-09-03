import { useState, useCallback, useRef, useEffect } from 'react';
import { App } from 'antd';
import { triggerParagraphTranslation, getParagraphTranslationStatus } from '../../../services/translationApi';

export interface PreTranslationProgress {
  translated: number;
  total: number;
  failed: number;
}

interface UsePreTranslationOptions {
  /**
   * 处于 generating 状态的论文 id 列表（调用方从 papers store 派生）。
   *
   * 上传 PDF 后后端自动触发的预翻译不经过 triggerPreTranslate，之前因此
   * 没有任何前端轮询，tag 永远显示"翻译中 0/0"（真实进度只在 DB 里）。
   * 这里自动接管这些 id，与手动触发走同一套轮询。
   */
  autoWatchIds?: string[];
  onStart?: (paperId: string) => void;
  onComplete?: (paperId: string, failed: number) => void;
}

/**
 * 预翻译进度轮询 hook
 *
 * 同时支持多篇论文在译（上传两份 PDF 会并行 generating）：
 * - pollingIds 是当前正在轮询的论文集合，共用一个 2s interval
 * - 手动触发（失败 tag 点击重试）与自动接管（autoWatchIds）都汇入同一集合
 * - 单篇到终态（done/failed）即移出集合并回调 onComplete，由调用方 patch store
 */
export function usePreTranslation(opts: UsePreTranslationOptions = {}) {
  const { message } = App.useApp();
  // 正在轮询进度的论文集合（ref 供 interval 内同步读取，state 驱动渲染）
  const [pollingIds, setPollingIds] = useState<ReadonlySet<string>>(new Set());
  const [progressMap, setProgressMap] = useState<Record<string, PreTranslationProgress>>({});
  const pollingIdsRef = useRef<Set<string>>(new Set());
  const inFlightRef = useRef(false);
  const optsRef = useRef(opts);
  optsRef.current = opts;

  const addPolling = useCallback((paperId: string) => {
    if (pollingIdsRef.current.has(paperId)) return;
    pollingIdsRef.current.add(paperId);
    setPollingIds(new Set(pollingIdsRef.current));
  }, []);

  const removePolling = useCallback((paperId: string) => {
    if (!pollingIdsRef.current.delete(paperId)) return;
    setPollingIds(new Set(pollingIdsRef.current));
    setProgressMap(prev => {
      if (!(paperId in prev)) return prev;
      const next = { ...prev };
      delete next[paperId];
      return next;
    });
  }, []);

  // 单篇论文拉一次进度；终态时移出轮询并回调 onComplete（由调用方 patch store，
  // 让 tag 立即从"翻译中"翻到"已翻译/翻译失败"，不等下一次列表刷新）
  const pollOne = useCallback(async (paperId: string) => {
    try {
      const status = await getParagraphTranslationStatus(paperId);
      const { translated, total, failed } = status.progress || {};
      setProgressMap(prev => ({
        ...prev,
        [paperId]: {
          translated: translated ?? 0,
          total: total ?? 0,
          failed: failed ?? 0,
        },
      }));

      if (status.status === 'completed' || status.status === 'done') {
        removePolling(paperId);
        const failedCount = failed ?? 0;
        if (failedCount > 0) {
          message.warning(`预翻译完成,但 ${failedCount} 段失败,可重试`);
        } else {
          message.success('预翻译完成');
        }
        optsRef.current.onComplete?.(paperId, failedCount);
      } else if (status.status === 'failed') {
        // 后端把整轮全失败标为 failed（一条都没翻出来），缺口即为失败段落数
        removePolling(paperId);
        message.error('预翻译失败,可点击重试');
        optsRef.current.onComplete?.(paperId, Math.max(1, (total ?? 0) - (translated ?? 0)));
      }
    } catch (e) {
      console.error('[PreTranslate] poll error:', e);
      removePolling(paperId);
    }
  }, [message, removePolling]);

  // 轮询循环：集合非空时启动 interval，清空时停止
  const hasPolling = pollingIds.size > 0;
  useEffect(() => {
    if (!hasPolling) return;
    const timer = setInterval(async () => {
      if (inFlightRef.current) return; // 上一轮未跑完（慢请求），跳过本 tick 防堆积
      inFlightRef.current = true;
      try {
        for (const id of Array.from(pollingIdsRef.current)) {
          await pollOne(id);
        }
      } finally {
        inFlightRef.current = false;
      }
    }, 2000);
    return () => clearInterval(timer);
  }, [hasPolling, pollOne]);

  // 自动接管 store 里处于 generating 的论文（上传 PDF 触发的预翻译路径）
  const { autoWatchIds } = opts;
  useEffect(() => {
    if (!autoWatchIds || autoWatchIds.length === 0) return;
    for (const id of autoWatchIds) addPolling(id);
  }, [autoWatchIds, addPolling]);

  // 手动触发（翻译失败 tag 点击重试）。轮询在 POST 返回前就挂上，
  // 让 tag 立即切到"翻译中"而不是等首个状态查询。
  const triggerPreTranslate = useCallback(async (paperId: string) => {
    if (pollingIdsRef.current.has(paperId)) return;
    addPolling(paperId);

    try {
      await triggerParagraphTranslation(paperId);
      message.info('已开始预翻译');
      optsRef.current.onStart?.(paperId);
    } catch (err: any) {
      console.error('[PreTranslate] trigger error:', err);
      removePolling(paperId);
      message.error(`预翻译失败: ${err.message || err}`);
    }
  }, [addPolling, removePolling, message]);

  const isPreTranslating = useCallback(
    (paperId: string) => pollingIds.has(paperId),
    [pollingIds],
  );

  return { isPreTranslating, progress: progressMap, triggerPreTranslate };
}
