/**
 * JayRead V2 导览 SSE 流式消费 Hook
 *
 * 用于消费后端 Server-Sent Events (SSE) 推送的导览生成进度和结果。
 * 替代 V1 的轮询机制，实现实时的生成进度反馈。
 *
 * 核心功能：
 * 1. EventSource 连接管理（自动重连）
 * 2. 多种 SSE 事件类型处理（guide_ready, section_refined, progress, done, error, timeout）
 * 3. 页面可见性变化管理（页面隐藏时暂停，恢复时重建）
 * 4. 清理逻辑（组件卸载时关闭连接）
 *
 * SSE 事件类型说明：
 * - guide_ready: 完整导览树生成完成，返回完整节点列表
 * - section_refined: 单个章节的精细化导览完成（Phase B）
 * - progress: 生成进度更新（当前阶段、进度百分比、预估剩余时间）
 * - done: 导览生成完成（成功结束）
 * - error: 生成失败（包含错误信息）
 * - timeout: 生成超时（长时间无响应）
 *
 * @module useGuideStream
 */

import { useState, useEffect, useCallback, useRef } from 'react'; // 导入 React 核心钩子
import { notifier } from '../bus';

/** 进度信息 */
interface ProgressInfo {
  phase: string | null;
  percent: number;
  etaMs: number | null;
  message: string;
}

/** useGuideStream 选项 */
interface UseGuideStreamOptions {
  enabled?: boolean;
  retryDelay?: number;
  maxRetries?: number;
  onNodeReady?: (nodes: unknown[]) => void;
  onProgress?: (progress: unknown) => void;
  onError?: (error: unknown) => void;
  onDone?: () => void;
}

/**
 * SSE 事件类型常量
 *
 * 与后端 guide_sse.py 发送的事件类型保持一致。
 */
const SSE_EVENTS = {
  GUIDE_READY: 'guide_ready', // 完整导览树生成完成
  SECTION_REFINED: 'section_refined', // 单个章节精细化导览完成
  PROGRESS: 'progress', // 生成进度更新
  DONE: 'done', // 生成完成
  ERROR: 'error', // 生成失败
  TIMEOUT: 'timeout', // 生成超时
};

/**
 * 连接状态常量
 *
 * 用于标识 SSE 连接的当前状态。
 */
const CONNECTION_STATUS = {
  CONNECTING: 'connecting', // 连接中（正在建立 EventSource 连接）
  STREAMING: 'streaming', // 流式传输中（已连接，正在接收数据）
  STREAMING_PHASE_A: 'streaming:phase_a', // Phase A 流式传输中（全文导览生成）
  STREAMING_PHASE_B: 'streaming:phase_b', // Phase B 流式传输中（精细化导览）
  DONE: 'done', // 完成（成功接收所有数据）
  ERROR: 'error', // 错误（连接失败或接收错误）
};

/**
 * V2 导览 SSE 流式消费 Hook
 *
 * @param {number} paperId - 论文 ID，用于构建 SSE 端点 URL
 * @param {object} options - 可选配置对象
 * @param {boolean} options.enabled - 是否启用 SSE 连接（默认 true），false 时不建立连接
 * @param {number} options.retryDelay - 重连延迟（毫秒），默认 3000ms
 * @param {number} options.maxRetries - 最大重连次数，默认 3 次
 * @param {Function} options.onNodeReady - 节点数据就绪回调：(nodes) => void
 * @param {Function} options.onProgress - 进度更新回调：(progress) => void
 * @param {Function} options.onError - 错误回调：(error) => void
 * @param {Function} options.onDone - 生成完成回调（后端 done 事件触发，用于 REST re-fetch）
 * @returns {object} Hook 返回的状态和方法
 *   - nodes: 导览节点数组（已就绪的节点）
 *   - status: 连接状态（CONNECTION_STATUS 枚举值）
 *   - error: 错误对象（如果有）
 *   - progress: 进度信息对象（当前阶段、百分比、剩余时间）
 *   - retry: 手动重连函数
 */
function useGuideStream(paperId: number, options: UseGuideStreamOptions = {}) {
  // 解构配置选项，设置默认值
  const {
    enabled = true, // 是否启用 SSE 连接
    retryDelay = 3000, // 重连延迟 3 秒
    maxRetries = 3, // 最多重连 3 次
    onNodeReady, // 节点就绪回调
    onProgress, // 进度更新回调
    onError, // 错误回调
    onDone, // 生成完成回调（后端 done 事件触发，用于 REST re-fetch）
  } = options;

  // ========== 状态管理 ==========

  const [nodes, setNodes] = useState<unknown[]>([]); // 状态：导览节点数组
  const [status, setStatus] = useState(CONNECTION_STATUS.CONNECTING); // 状态：连接状态
  const [error, setError] = useState<Error | null>(null); // 状态：错误对象
  const [progress, setProgress] = useState({
    phase: null,
    percent: 0,
    etaMs: null,
    message: '',
  }); // 状态：进度信息对象

  // ========== Refs（用于跨闭包访问）==========

  const eventSourceRef = useRef<EventSource | null>(null); // ref：EventSource 实例
  const retryCountRef = useRef(0); // ref：重连次数计数器
  const manuallyClosedRef = useRef(false); // ref：手动关闭标志
  const cooldownTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null); // ref：冷却定时器
  const statusRef = useRef(status); // ref：最新连接状态（供可见性 effect 读取，避免将其加入依赖数组）
  statusRef.current = status;

  // Ref for closeConnection to avoid dependency issues (will be assigned after closeConnection is defined)
  const closeConnectionRef = useRef<() => void>(() => {});

  /**
   * 用 refs 存储最新的回调函数
   *
   * 这样事件处理函数始终读取最新的回调，而不需要把回调放入
   * useCallback 的依赖数组，避免 connect 因回调引用变化而重建。
   */
  const callbacksRef = useRef<UseGuideStreamOptions>({});
  callbacksRef.current = { onNodeReady, onProgress, onError, onDone };

  // ========== 内部函数 ==========

  /**
   * 关闭 SSE 连接
   *
   * 清理 EventSource 实例，释放资源。
   */
  const closeConnection = useCallback(() => {
    if (eventSourceRef.current) {
      // 设置手动关闭标志，避免触发重连逻辑
      manuallyClosedRef.current = true;

      // Clear event handlers to prevent listener accumulation
      eventSourceRef.current.onerror = null;
      eventSourceRef.current.onopen = null;
      eventSourceRef.current.onmessage = null;

      // 关闭 EventSource 连接
      eventSourceRef.current.close();
      eventSourceRef.current = null; // 清空引用
    }
    // 清除冷却定时器
    if (cooldownTimerRef.current) {
      clearTimeout(cooldownTimerRef.current);
      cooldownTimerRef.current = null;
    }
  }, []); // 无依赖项

  // Update the ref after closeConnection is defined
  closeConnectionRef.current = closeConnection;

  /**
   * 建立 SSE 连接
   *
   * 创建 EventSource 实例并注册事件监听器。
   * 事件处理函数通过 callbacksRef.current 读取最新回调，
   * 因此 connect 的依赖数组不包含任何回调函数，保持引用稳定。
   */
  const connect = useCallback(() => {
    // 检查是否禁用
    if (!enabled) {
      return;
    }

    // 已完成或已出错时不重连（页面可见性恢复等场景）
    if (statusRef.current === CONNECTION_STATUS.DONE || statusRef.current === CONNECTION_STATUS.ERROR) {
      return;
    }

    // 检查是否已存在连接
    if (eventSourceRef.current) {
      console.warn('[useGuideStream] SSE 连接已存在，跳过重复连接');
      return;
    }

    // 重置手动关闭标志（允许重连）
    manuallyClosedRef.current = false;

    // 构建 SSE 端点 URL
    const url = `/api/papers/${paperId}/guide/stream`;

    try {
      // 创建 EventSource 实例
      const eventSource = new EventSource(url);
      eventSourceRef.current = eventSource;

      // 更新连接状态
      setStatus(CONNECTION_STATUS.CONNECTING);

      // 读取当前最新回调的本地引用（用于整个连接生命周期）
      const { onNodeReady: curOnNodeReady, onProgress: curOnProgress, onError: curOnError, onDone: curOnDone } = callbacksRef.current;

      // ========== 注册事件监听器 ==========

      // guide_ready: 完整导览树生成完成
      eventSource.addEventListener(SSE_EVENTS.GUIDE_READY, (event) => {
        try {
          const data = JSON.parse(event.data);
          curOnProgress?.({ phase: 'phase_a', percent: 50, message: '全文导览生成完成，等待精细化...' });
        } catch (err) {
          console.error('[useGuideStream] 解析 guide_ready 事件失败:', err);
        }
      });

      // section_refined: 单个章节精细化导览完成
      eventSource.addEventListener(SSE_EVENTS.SECTION_REFINED, (event) => {
        try {
          const data = JSON.parse(event.data);
          curOnProgress?.({ phase: 'phase_b', message: `章节 "${data.section_title || ''}" 精细化完成` });
        } catch (err) {
          console.error('[useGuideStream] 解析 section_refined 事件失败:', err);
        }
      });

      // progress: 生成进度更新
      eventSource.addEventListener(SSE_EVENTS.PROGRESS, (event) => {
        try {
          const data = JSON.parse(event.data);

          const { phase, percent, eta_ms, message } = data;

          setProgress({
            phase: phase || null,
            percent: percent || 0,
            etaMs: eta_ms || null,
            message: message || '',
          });

          if (phase === 'phase_a') {
            setStatus(CONNECTION_STATUS.STREAMING_PHASE_A);
          } else if (phase === 'phase_b') {
            setStatus(CONNECTION_STATUS.STREAMING_PHASE_B);
          }

          curOnProgress?.(data);
        } catch (err) {
          console.error('[useGuideStream] 解析 progress 事件失败:', err);
        }
      });

      // done: 导览生成完成
      eventSource.addEventListener(SSE_EVENTS.DONE, (event) => {
        setStatus(CONNECTION_STATUS.DONE);

        // 关闭 SSE 连接
        if (eventSourceRef.current === eventSource) {
          manuallyClosedRef.current = true;
          eventSource.close();
          eventSourceRef.current = null;
        }

        // 通知其他模块：导览生成完成
        notifier.trigger('modify', 'paper', [String(paperId)], { guideStatus: 'completed' });

        curOnDone?.();
      });

      // error: 生成失败
      eventSource.addEventListener(SSE_EVENTS.ERROR, (event) => {
        try {
          const data = JSON.parse(event.data);
          console.error('[useGuideStream] error 事件:', data);

          const errorMessage = data.message || '导览生成失败';

          setError(new Error(errorMessage));
          setStatus(CONNECTION_STATUS.ERROR);

          curOnError?.(data);

          // 通知其他模块：导览生成失败
          notifier.trigger('modify', 'paper', [String(paperId)], { guideStatus: 'error', error: errorMessage });

          if (eventSourceRef.current === eventSource) {
            manuallyClosedRef.current = true;
            eventSource.close();
            eventSourceRef.current = null;
          }
        } catch (err) {
          console.error('[useGuideStream] 解析 error 事件失败:', err);
          setError(new Error('解析错误信息失败'));
          setStatus(CONNECTION_STATUS.ERROR);
          if (eventSourceRef.current === eventSource) {
            manuallyClosedRef.current = true;
            eventSource.close();
            eventSourceRef.current = null;
          }
        }
      });

      // timeout: 生成超时
      eventSource.addEventListener(SSE_EVENTS.TIMEOUT, (event) => {
        console.warn('[useGuideStream] timeout 事件:', event.data);

        setError(new Error('导览生成超时，请稍后重试'));
        setStatus(CONNECTION_STATUS.ERROR);

        curOnError?.({ message: '生成超时', code: 'TIMEOUT' });

        if (eventSourceRef.current === eventSource) {
          manuallyClosedRef.current = true;
          eventSource.close();
          eventSourceRef.current = null;
        }
      });

      // onopen: 连接建立成功
      eventSource.onopen = () => {
        setStatus(CONNECTION_STATUS.STREAMING);
        // 重置重连计数
        retryCountRef.current = 0;
      };

      // onerror: 连接错误（自动重连）
      eventSource.onerror = (err) => {
        console.error('[useGuideStream] SSE 连接错误:', err);

        // 检查是否手动关闭
        if (manuallyClosedRef.current) {
          return;
        }

        // 检查重连次数限制
        if (retryCountRef.current < maxRetries) {
          retryCountRef.current += 1;

          // 延迟后重连
          setTimeout(() => {
            if (eventSourceRef.current === eventSource) {
              manuallyClosedRef.current = true;
              eventSource.close();
              eventSourceRef.current = null;
              // 直接调用 connect 的最新版本（通过 connectRef）
              connectRef.current();
            }
          }, retryDelay);
        } else {
          console.error('[useGuideStream] 已达到最大重连次数，进入冷却期');
          setError(new Error('SSE 连接失败，30秒后将自动重试'));
          setStatus(CONNECTION_STATUS.ERROR);
          if (eventSourceRef.current === eventSource) {
            manuallyClosedRef.current = true;
            eventSource.close();
            eventSourceRef.current = null;
          }
          // 30秒后冷却，重置重试计数并尝试重新连接
          cooldownTimerRef.current = setTimeout(() => {
            console.log('[useGuideStream] 冷却期结束，重置重连计数');
            retryCountRef.current = 0;
            cooldownTimerRef.current = null;
            setError(null);
            connectRef.current();
          }, 30000);
        }
      };
    } catch (err) {
      console.error('[useGuideStream] 创建 EventSource 失败:', err);
      setError(new Error('无法建立 SSE 连接'));
      setStatus(CONNECTION_STATUS.ERROR);
    }
  }, [enabled, paperId, maxRetries, retryDelay]); // 仅依赖原始值，不依赖回调函数

  /**
   * connect 的 ref，用于在 onerror 重连时调用最新的 connect
   */
  const connectRef = useRef<() => void>(connect);
  connectRef.current = connect;

  /**
   * 手动重连函数
   *
   * 允许用户主动触发重连，不受重连次数限制。
   * 用于用户点击"重试"按钮的场景。
   */
  const retry = useCallback(() => {
    // 重置状态
    setError(null);
    setStatus(CONNECTION_STATUS.CONNECTING);
    retryCountRef.current = 0; // 重置重连计数

    // 关闭旧连接（如果存在）
    closeConnection();

    // 建立新连接
    connect();
  }, [closeConnection, connect]); // closeConnection 和 connect 引用稳定

  // ========== 副作用（Effects）==========

  /**
   * Effect: 建立初始 SSE 连接
   *
   * 当 paperId 或 enabled 变化时，重新建立连接。
   */
  useEffect(() => {
    // 建立连接
    connect();

    // 清理函数：组件卸载时关闭连接
    return () => {
      closeConnection();
    };
  }, [paperId, enabled]); // 只依赖 paperId 和 enabled，不依赖 connect

  /**
   * Effect: 页面可见性变化管理
   *
   * 当页面隐藏时暂停 SSE 连接，恢复时重建连接。
   * 避免后台页面占用服务器资源。
   */
  useEffect(() => {
    // 处理页面可见性变化
    const handleVisibilityChange = () => {
      if (document.hidden) {
        // 页面隐藏：关闭连接
        closeConnectionRef.current();
      } else {
        // 页面恢复：重建连接
        // 直接读取最新状态判断是否需要重建
        connectRef.current();
      }
    };

    // 注册可见性变化监听器
    document.addEventListener('visibilitychange', handleVisibilityChange);

    // 清理函数：移除监听器
    return () => {
      document.removeEventListener('visibilitychange', handleVisibilityChange);
    };
  }, []); // 空依赖数组：使用 ref 避免重复注册监听器

  // ========== 返回值 ==========

  return {
    // 数据
    nodes, // 导览节点数组（已就绪的节点）

    // 状态
    status, // 连接状态（CONNECTION_STATUS 枚举值）
    error, // 错误对象（如果有）

    // 进度
    progress, // 进度信息对象（当前阶段、百分比、剩余时间）

    // 方法
    retry, // 手动重连函数
  };
}

// ========== 导出 ==========

export default useGuideStream;
export { CONNECTION_STATUS };
