/**
 * 嵌入状态轮询 Hook（已桩化）
 *
 * autonomics 后端没有向量嵌入管线（/embed-status、/embed 端点不存在）。
 * jayread 原实现对 404 的 JSON 错误体解析出 undefined 字段、被误判为
 * pending，进而陷入每 3 秒一次的 404 轮询 + 无意义的 /embed POST ——
 * 这里整体桩化为永返 null（聊天区的嵌入等待横幅永不出现）。
 *
 * 保留导出签名（EmbeddingStatus 类型与返回形状）以维持 ChatPanel
 * 调用点零改动；死代码清扫阶段可与横幅 UI 一并移除。
 *
 * @module ai-chat/hooks/chat-side-effects/useEmbeddingStatus
 */

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
 * 嵌入状态轮询 Hook（桩化实现）
 *
 * @returns embeddingStatus 恒为 null（无需展示横幅）
 */
export function useEmbeddingStatus(_params: UseEmbeddingStatusParams): {
  embeddingStatus: EmbeddingStatus | null;
} {
  return { embeddingStatus: null };
}
