/**
 * useSSEProgress - 论文解析进度监听钩子（已桩化）
 *
 * jayread 的上传是异步解析流水线（MinerU CLI），前端靠 SSE 接收
 * progress / done / error 推送。autonomics 的 `POST /articles/upload`
 * 在服务端**同步**完成文本抽取 + 建档，返回即终态——不存在解析中的
 * 论文，也就不需要 SSE。
 *
 * 保留 hook 与返回签名（listenToParseStatus / reconnectPendingPapers /
 * cleanup）使 PaperListPage 及 useBibliographyImport 等调用点零改动；
 * 三个函数均为 no-op。死代码清扫阶段可连同调用点一并移除。
 *
 * @module PaperListPage/hooks/useSSEProgress
 */

import { useCallback } from 'react'; // 导入 React 核心钩子
import type { Paper } from '@/types';

/** 进度数据（保留类型，供调用方回调签名引用） */
interface ProgressData {
  paperId: string;
  percent: number;
  stage: string;
}

/** 完成数据（保留类型，供调用方回调签名引用） */
interface DoneData {
  paperId: string;
  parseEngine: string;
  markdownLength: number;
}

/** 错误数据（保留类型，供调用方回调签名引用） */
interface ErrorData {
  paperId: string;
  message: string;
}

/** useSSEProgress 参数 */
interface UseSSEProgressParams {
  onProgress?: (data: ProgressData) => void;
  onDone?: (data: DoneData) => void;
  onError?: (data: ErrorData) => void;
}

/**
 * 解析进度监听 Hook（桩化实现，全部 no-op）
 *
 * @returns 与原实现相同的三个函数：listenToParseStatus / reconnectPendingPapers / cleanup
 */
export function useSSEProgress(_params: UseSSEProgressParams) {
  /** no-op：autonomics 无异步解析，无需监听 */
  const listenToParseStatus = useCallback((_paperId: string) => {}, []);

  /** no-op：不存在 parse_status 停留在解析中的论文，无需重连 */
  const reconnectPendingPapers = useCallback(
    (_papers: Paper[], _loading: boolean, hasReconnectedRef: React.MutableRefObject<boolean>) => {
      // 保持原语义：调用方期望这个 ref 被置位以避免反复触发
      hasReconnectedRef.current = true;
    },
    [],
  );

  /** no-op：没有需要清理的连接 */
  const cleanup = useCallback(() => {}, []);

  return {
    listenToParseStatus,    // 开始监听论文解析状态（no-op）
    reconnectPendingPapers, // 重连正在解析的论文（no-op）
    cleanup,                // 手动清理所有连接（no-op）
  };
}
