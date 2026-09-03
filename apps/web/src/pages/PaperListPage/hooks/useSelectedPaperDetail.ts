/**
 * useSelectedPaperDetail - 选中文献的详情获取
 *
 * 监听 selectedPaper 变化，调 `getPaper(id)` 拉取完整详情快照（含 abstract）。
 *
 * 注：jayread 时代这里有 paragraphTranslationStatus 轮询；autonomics 无翻译
 * 管线，mapping 恒把该字段映射为 null，轮询分支永不触发，已随 translationApi
 * 一并移除（plan Phase 5 清扫）。
 */
import { useState, useEffect } from 'react';
import type { Paper } from '@/types';
import { getPaper } from '../../../services/papersApi';

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

    const pull = async () => {
      try {
        const result = await getPaper(selectedPaper.id, controller.signal);
        setDetail(result);
        setLoading(false);
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
    };
  }, [selectedPaper]);

  return { detail, loading };
}
