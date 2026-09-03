/**
 * useMetadataPolling - 元数据获取轮询钩子
 *
 * 当后端异步获取论文元数据（IF、分区、被引次数）时，
 * 前端通过轮询 GET /api/metadata/pending 检测获取是否完成。
 * 完成后自动调用 loadPapers() 刷新列表，用户无需手动刷新页面。
 *
 * 使用场景：
 * 1. 题录导入后，批量获取元数据
 * 2. 手动点击"获取期刊信息"
 * 3. 去重解决后触发元数据获取
 *
 * 设计参考：useGuideGeneration.js 的轮询机制（setInterval + maxPolls 超时保护）
 *
 * 为什么用轮询而非 SSE？
 * - 题录导入的论文 parse_status 为 "done"（无 PDF 需解析），
 *   现有 SSE 端点 GET /api/papers/{id}/stream 会立即返回并关闭连接，
 *   无法用于元数据获取通知。
 * - 轮询实现简单可靠，无需修改 SSE 基础设施。
 */

import { useRef, useCallback, useEffect } from 'react'; // 导入 React 核心钩子
import { checkMetadataPending } from '../services/papersApi'; // 导入元数据状态查询 API

/**
 * 轮询控制器引用
 * 用于在组件卸载或启动新轮询时取消进行中的请求
 */

/**
 * 元数据获取轮询钩子
 *
 * @param {Function} loadPapers - 刷新论文列表的回调函数（从父组件传入）
 * @returns {Object} 返回包含以下属性的对象：
 *   - startMetadataPolling: 启动轮询的函数
 *   - cleanup: 清理轮询定时器的函数（供组件卸载时调用）
 */
export function useMetadataPolling(loadPapers: () => void) {
  // 轮询定时器引用：存储 setInterval 返回的定时器 ID
  // 使用 ref 而非 state，因为定时器 ID 不需要触发组件重新渲染
  const pollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // AbortController 引用：用于取消进行中的轮询请求
  const abortControllerRef = useRef<AbortController | null>(null);

  /**
   * 启动元数据获取状态轮询
   *
   * 调用此函数后，前端会每隔 interval 毫秒向后端查询指定论文的元数据获取状态。
   * 当所有论文的元数据都获取完成后，自动停止轮询并刷新论文列表。
   *
   * @param {number[]} paperIds - 需要轮询的论文 ID 数组
   * @param {object} [options] - 可选配置对象
   * @param {number} [options.interval=3000] - 轮询间隔（毫秒），默认 3 秒
   *   3 秒在及时发现完成和避免频繁请求之间取得平衡
   * @param {number} [options.maxPolls=10] - 最大轮询次数，默认 10 次
   *   10 次 × 3 秒 = 30 秒超时，足够覆盖 OpenAlex + easyScholar 的 API 调用时间
   */
  const startMetadataPolling = useCallback((paperIds: number[], options: { interval?: number; maxPolls?: number } = {}) => {
    // 解构配置参数，设置默认值
    const { interval = 3000, maxPolls = 10 } = options;

    // 如果没有需要追踪的论文 ID，直接返回，不启动轮询
    if (!paperIds || paperIds.length === 0) return;

    // 如果已有活跃的轮询定时器，先清除（防止重复轮询）
    // 这种情况可能在用户快速连续触发"获取期刊信息"时发生
    if (pollTimerRef.current) {
      clearInterval(pollTimerRef.current);
      pollTimerRef.current = null;
    }

    // 取消之前的轮询请求（如果有）
    if (abortControllerRef.current) {
      abortControllerRef.current.abort();
      abortControllerRef.current = null;
    }

    // 轮询次数计数器，每次执行时自增
    let pollCount = 0;
    // 复制一份 ID 列表，轮询过程中逐步缩小（已完成的 ID 从列表中移除）
    let remainingIds = [...paperIds];

    // 创建 AbortController 用于取消轮询请求
    const controller = new AbortController();
    abortControllerRef.current = controller;

    // 使用递归 setTimeout 替代 setInterval，避免异步回调重叠
    // setInterval 在回调耗时超过间隔时会累积重叠请求
    const schedulePoll = () => {
      pollTimerRef.current = setTimeout(async () => {
        // Check abortSignal immediately at the start (cleanup race fix)
        if (controller.signal.aborted) {
          return;
        }

        pollCount++; // 轮询次数加一

        try {
          // 向后端发送 GET 请求，查询哪些论文仍在获取元数据
          const result = await checkMetadataPending(controller.signal);

          if (result.pending_ids.length === 0) {
            // 所有论文的元数据都已获取完成（成功或失败），停止轮询
            pollTimerRef.current = null;
            abortControllerRef.current = null;
            // 刷新论文列表，显示最新的元数据（IF、被引、分区等列将更新）
            if (controller.signal.aborted) return;
            loadPapers();
            return;
          } else {
            // 部分论文仍在获取中，更新剩余 ID 列表
            // 减少下次请求的 ID 数量，降低查询开销
            remainingIds = result.pending_ids as unknown as number[];
          }

          // 超过最大轮询次数（30 秒），强制停止轮询
          if (pollCount >= maxPolls) {
            pollTimerRef.current = null;
            abortControllerRef.current = null;
            // 超时时仍然刷新一次列表，显示已获取到的部分数据
            // 未完成的论文可能在后续操作中自然获取完成
            if (controller.signal.aborted) return;
            loadPapers();
            return;
          }

          // 请求完成后才调度下一次轮询，确保不会重叠
          schedulePoll();
        } catch (err) {
          // 忽略 AbortError，只记录其他错误并继续轮询
          if ((err as Error).name !== 'AbortError') {
            // 单次轮询失败（网络错误、后端重启等），忽略并继续轮询
            // 如果后端持续不可用，最终 pollCount 会达到 maxPolls 上限，触发超时兜底逻辑
            if (pollCount < maxPolls) {
              schedulePoll();
            }
          }
        }
      }, interval);
    };

    // 启动首次轮询
    schedulePoll();
  }, [loadPapers]); // 依赖项：loadPapers 函数

  /**
   * 清理轮询定时器
   *
   * 在以下情况调用：
   * 1. 组件卸载时（通过 useEffect cleanup）
   * 2. 新轮询启动时（在 startMetadataPolling 内部自动清理旧定时器）
   *
   * 不清理会导致：
   * - 组件卸载后定时器仍在执行，尝试更新已卸载的组件（内存泄漏）
   * - 多个定时器同时运行，发送重复请求
   */
  const cleanup = useCallback(() => {
    if (pollTimerRef.current) {
      clearTimeout(pollTimerRef.current); // 清除 setTimeout 定时器
      pollTimerRef.current = null; // 重置引用为 null
    }
    if (abortControllerRef.current) {
      abortControllerRef.current.abort(); // 取消进行中的请求
      abortControllerRef.current = null; // 重置引用为 null
    }
  }, []);

  // 组件卸载时自动清理轮询定时器，防止内存泄漏
  useEffect(() => {
    return cleanup; // 返回清理函数，React 会在组件卸载时调用
  }, [cleanup]);

  // 返回公共接口
  return {
    startMetadataPolling, // 启动元数据获取状态轮询
    cleanup,              // 手动清理轮询定时器
  };
}
