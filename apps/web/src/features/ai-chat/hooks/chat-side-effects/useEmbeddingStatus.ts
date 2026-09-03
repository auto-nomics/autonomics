/**
 * 嵌入状态轮询 Hook
 *
 * 当论文解析完成后，定期轮询检查其嵌入（向量化）状态。
 * 如果嵌入未完成，在聊天区域顶部显示提示条，告知用户 RAG 检索功能暂不可用。
 * 轮询间隔为 3 秒，嵌入完成后停止轮询。
 *
 * 抽出原因：原 ChatPanel 的副作用块（~70 行）与消息/UI 编排无关，
 * 独立成 hook 让 ChatPanel 专注于对话编排。
 *
 * 注意：KMS 结构化提取已替代向量嵌入作为主要检索路径，
 * 但旧论文的 embed-status 仍可能显示等待横幅。当 pending 且从未 done 时，
 * 视为不需要旧嵌入，直接清空状态（不显示横幅）。
 *
 * @module ai-chat/hooks/chat-side-effects/useEmbeddingStatus
 */

import { useEffect, useRef, useState } from 'react';

/** 嵌入状态（适配自 Rust 后端 { pending, done, total } 格式） */
export interface EmbeddingStatus {
  status: 'idle' | 'pending' | 'in_progress' | 'done';
  embedded: number;
  totalChunks: number;
}

/** useEmbeddingStatus 参数 */
interface UseEmbeddingStatusParams {
  paperId: string | number | null;
  /** 论文 parse_status，仅 'done' 时启动轮询 */
  parseStatus?: string;
}

/**
 * 嵌入状态轮询 Hook
 *
 * - paperId 缺失或 parse_status !== 'done' 时清空状态、不启动轮询
 * - 首次检测到 pending 时自动 POST /embed 触发后端嵌入（仅触发一次/paperId）
 *
 * @returns embeddingStatus 当前嵌入状态（null = 无需展示横幅）
 */
export function useEmbeddingStatus({ paperId, parseStatus }: UseEmbeddingStatusParams): {
  embeddingStatus: EmbeddingStatus | null;
} {
  const [embeddingStatus, setEmbeddingStatus] = useState<EmbeddingStatus | null>(null);
  // 记录已触发嵌入的 paperId，避免重复 POST /embed
  const embedTriggeredRef = useRef<string | number | null>(null);

  useEffect(() => {
    if (!paperId || parseStatus !== 'done') {
      setEmbeddingStatus(null);
      return;
    }

    let timeoutId: ReturnType<typeof setTimeout> | null = null;
    let cancelled = false;

    const poll = () => {
      fetch(`/api/papers/${paperId}/embed-status`)
        .then(r => r.json())
        .then(data => {
          if (cancelled) return;

          const adapted: EmbeddingStatus = {
            status: data.total === 0 ? 'idle'
              : data.pending === 0 ? 'done'
              : data.pending === data.total ? 'pending'
              : 'in_progress',
            embedded: data.done,
            totalChunks: data.total,
          };

          // 全部 pending 且从未 done：KMS 已替代，不需要显示等待横幅
          if (adapted.status === 'pending' && data.done === 0) {
            setEmbeddingStatus(null);
            return;
          }

          setEmbeddingStatus(adapted);

          // 首次检测到 pending 时自动触发嵌入（每个 paperId 仅触发一次）
          if (adapted.status === 'pending' && embedTriggeredRef.current !== paperId) {
            embedTriggeredRef.current = paperId;
            fetch(`/api/papers/${paperId}/embed`, { method: 'POST' }).catch(() => {});
          }

          if (adapted.status !== 'done' && adapted.status !== 'idle') {
            timeoutId = setTimeout(poll, 3000);
          }
        })
        .catch(() => {
          if (!cancelled) setEmbeddingStatus(null);
        });
    };

    poll();

    return () => {
      cancelled = true;
      if (timeoutId !== null) clearTimeout(timeoutId);
    };
  }, [paperId, parseStatus]);

  return { embeddingStatus };
}
