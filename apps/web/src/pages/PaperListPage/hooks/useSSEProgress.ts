/**
 * useSSEProgress - 论文解析进度监听钩子（MinerU 异步管线）
 *
 * PDF 上传后后端立即返回 pending，由后台任务调 MinerU 云 API 解析；
 * 前端为每篇论文开一条 SSE 流接收推送：
 * - progress: {percent, stage}（上传 5% → 页进度 10-90%）
 * - done:     {parse_engine, markdown_length}（终态，关连接）
 * - error:    {message}（终态，关连接，用户可重解析）
 * - ping:     10s 心跳，忽略
 *
 * 为什么是 fetch 读取器而不是 EventSource？
 * - tui-http 的 bearer 鉴权层拦 /api/**，而 EventSource 设不了 Authorization
 *   头；本地无 token 时裸 fetch 也无害（不附带头即可）
 * - EventSource 的自动重连不可控；fetch 版可以精确区分「网络断开要重连」
 *   和「业务终态不重连」
 *
 * 语义沿用 jayread（EventSource 版）：
 * - 每论文一连接，重复 listen 先断旧连
 * - 网络断开指数退避重试：1s → 2s → 4s，最多 3 次，之后等用户手动重解析
 * - errorHandled 标记区分业务错误（不重试）与网络异常（重试）
 * - done/error 后关连接；组件卸载时自动清理全部连接与定时器
 *
 * @module PaperListPage/hooks/useSSEProgress
 */

import { useRef, useCallback, useEffect } from 'react'; // 导入 React 核心钩子
import { App } from 'antd'; // Ant Design App 上下文（主题正确的 message 实例）
import { useTranslation } from 'react-i18next'; // i18n 翻译钩子
import type { Paper } from '@/types';
import { getStreamUrl } from '../../../services/papersApi'; // SSE 端点 URL（同源相对路径）
import { getAuthToken } from '../../../services/client'; // bearer token（与 request() 同源）

/** 进度数据 */
interface ProgressData {
  paperId: string;
  percent: number;
  stage: string;
}

/** 完成数据 */
interface DoneData {
  paperId: string;
  parseEngine: string;
  markdownLength: number;
}

/** 错误数据 */
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
 * SSE 进度监听钩子
 *
 * @param {Function} onProgress - 进度更新回调：({paperId, percent, stage})
 * @param {Function} onDone - 解析完成回调：({paperId, parseEngine, markdownLength})
 * @param {Function} onError - 解析失败回调：({paperId, message})
 * @returns {Object} listenToParseStatus / reconnectPendingPapers / cleanup
 */
export function useSSEProgress({ onProgress, onDone, onError }: UseSSEProgressParams) {
  const { message } = App.useApp();
  const { t } = useTranslation('paperList');

  // SSE 连接注册表：每篇论文一条流，值是该次连接的 AbortController。
  // jayread 存 EventSource（close 即断），fetch 版对应的「close」是 abort。
  const sseRefs = useRef<Record<string, AbortController>>({});

  // SSE 重试计数器：{paperId: 已重试次数}，指数退避的指数来源
  const sseRetryCount = useRef<Record<string, number>>({});

  // SSE 重试定时器注册表：{paperId: setTimeout ID}，卸载时清理防泄漏
  const sseRetryTimers = useRef<Record<string, ReturnType<typeof setTimeout>>>({});

  // 最新回调：读循环是长生命周期闭包，直接捕获参数会在回调身份变化后失联，
  // 经 ref 取值保证每次事件都调到当前渲染的回调
  const callbacksRef = useRef({ onProgress, onDone, onError });
  useEffect(() => {
    callbacksRef.current = { onProgress, onDone, onError };
  });

  // 自引用：重试定时器经 ref 调最新版 listen，避免捕获旧闭包
  const listenRef = useRef<(paperId: string) => void>(() => {});

  /**
   * 监听论文解析状态的 SSE 推送
   *
   * @param {string} paperId - 需要监听的论文 ID（safeId）
   */
  const listenToParseStatus = useCallback((paperId: string) => {
    // 该论文已有连接先断开（快速重复触发重解析时防双连接）。
    // abort 让旧读循环在下一个微任务里醒来，届时注册表已指向新连接，
    // 旧循环的身份守卫会直接放行退出
    sseRefs.current[paperId]?.abort();

    const controller = new AbortController();
    sseRefs.current[paperId] = controller;

    // 业务错误标记：后端 error 事件处理后连接由我们主动关闭，
    // 随后的流中断不应进入网络重试（对应 jayread 的 errorHandled）
    let errorHandled = false;

    /** 终态收尾：断开连接、清理注册表与重试计数 */
    const finish = () => {
      controller.abort(); // 幂等；打断挂起的 reader.read()
      if (sseRefs.current[paperId] === controller) delete sseRefs.current[paperId];
      delete sseRetryCount.current[paperId];
    };

    /**
     * 处理一个完整 SSE 帧
     * @returns 是否终态事件（done/error）——true 时读循环应结束
     */
    const handleFrame = (event: string, data: string): boolean => {
      try {
        if (event === 'progress') {
          const parsed = JSON.parse(data);
          callbacksRef.current.onProgress?.({
            paperId,
            percent: typeof parsed.percent === 'number' ? parsed.percent : 0,
            stage: parsed.stage || '',
          });
          return false;
        }
        if (event === 'done') {
          const parsed = JSON.parse(data);
          callbacksRef.current.onDone?.({
            paperId,
            parseEngine: parsed.parse_engine,
            markdownLength: parsed.markdown_length,
          });
          message.success(t('message.reparseDone')); // 弹出成功提示
          finish();
          return true;
        }
        if (event === 'error') {
          errorHandled = true; // 合法解析错误，关流后不进重试
          const parsed = JSON.parse(data);
          callbacksRef.current.onError?.({ paperId, message: parsed.message });
          message.error(parsed.message || t('status.failed')); // 弹出错误提示
          finish();
          return true;
        }
        // ping 心跳与未知事件：忽略
        return false;
      } catch (err) {
        console.warn('[SSE] Failed to parse event data:', data, err);
        return false;
      }
    };

    /**
     * 网络断开路径（fetch 拒绝 / 非 2xx / 流关闭未见终态）
     *
     * 重试策略与 jayread 一致：指数退避 1s → 2s → 4s，最多 3 次，
     * 超过则放弃，等待用户手动触发（如右键「重解析」）
     */
    const onDrop = () => {
      if (errorHandled) return; // 业务错误已处理，不重连
      // 身份守卫：注册表里的连接已不是本次（被清理或被新 listen 取代），
      // 说明中断是有意的，不重试
      if (sseRefs.current[paperId] !== controller) return;
      delete sseRefs.current[paperId];

      const currentRetries = sseRetryCount.current[paperId] || 0;
      if (currentRetries < 3) {
        sseRetryCount.current[paperId] = currentRetries + 1;
        const delay = Math.pow(2, currentRetries) * 1000; // 2^0=1s, 2^1=2s, 2^2=4s
        sseRetryTimers.current[paperId] = setTimeout(() => {
          delete sseRetryTimers.current[paperId];
          listenRef.current(paperId); // 经 ref 调最新版重建连接
        }, delay);
      }
    };

    // 读循环：拉流 → 解码 → 按空行切帧 → 逐行解析 event:/data:
    void (async () => {
      try {
        const headers: Record<string, string> = { Accept: 'text/event-stream' };
        const token = getAuthToken();
        if (token) headers.Authorization = `Bearer ${token}`;

        const res = await fetch(getStreamUrl(paperId), { headers, signal: controller.signal });
        if (!res.ok || !res.body) {
          onDrop(); // 404/401 等：与 EventSource onerror 同语义，走有上限的重试
          return;
        }

        const reader = res.body.getReader();
        const decoder = new TextDecoder();
        let buffer = '';
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break; // 流关闭且未见终态 → 网络断开
          // 归一 CRLF 后累积；残帧（无尾部空行）留在 buffer 等下一块
          buffer = (buffer + decoder.decode(value, { stream: true })).replace(/\r\n/g, '\n');
          for (;;) {
            const sep = buffer.indexOf('\n\n');
            if (sep < 0) break;
            const frame = buffer.slice(0, sep);
            buffer = buffer.slice(sep + 2);
            // 帧内逐行：event: 定事件名，data: 累积数据（多行按 SSE 规范以 \n 连接）
            let event = 'message';
            const dataLines: string[] = [];
            for (const line of frame.split('\n')) {
              if (line.startsWith('event:')) event = line.slice(6).trim();
              else if (line.startsWith('data:')) dataLines.push(line.slice(5).trim());
            }
            if (dataLines.length === 0) continue;
            if (handleFrame(event, dataLines.join('\n'))) return; // 终态：连接已在 finish() 关闭
          }
        }
        onDrop();
      } catch {
        // AbortError（finish/清理/换新连接的主动断开）也落这里，
        // onDrop 的 errorHandled 与身份守卫负责放行有意中断
        onDrop();
      }
    })();
  }, [message, t]);

  // 保持自引用指向最新版（依赖回调身份稳定时本身就稳定）
  useEffect(() => {
    listenRef.current = listenToParseStatus;
  }, [listenToParseStatus]);

  /**
   * 初始 SSE 重连：页面加载/刷新后，为正在解析的论文自动重建 SSE 监听
   *
   * 覆盖场景：
   * 1. 解析中刷新页面 → 重建 SSE → 进度恢复实时更新
   * 2. 解析完成后才回来 → loadPapers 已返回 done → 不重连
   * 3. 后端重启丢任务 → 打开 SSE 会触发 ensure_running 懒恢复（服务端语义）
   *
   * @param {Array} papers - 论文列表
   * @param {boolean} loading - 是否正在加载（未加载完跳过）
   * @param {Object} hasReconnectedRef - 重连标记 ref，控制只执行一次
   */
  const reconnectPendingPapers = useCallback((papers: Paper[], loading: boolean, hasReconnectedRef: React.MutableRefObject<boolean>) => {
    if (loading) return; // 等 loadPapers 完成后再检查

    if (hasReconnectedRef.current) return; // 已执行过初始重连，跳过
    hasReconnectedRef.current = true; // 标记为已执行

    // 'parsing' 是前端 SSE 期间的乐观态；'processing'/'pending' 是后端存储值
    papers.forEach(p => {
      if ((p.parse_status === 'parsing' || p.parse_status === 'processing' || p.parse_status === 'pending')
        && !sseRefs.current[p.id]) {
        listenToParseStatus(p.id);
      }
    });
  }, [listenToParseStatus]);

  /**
   * 清理所有 SSE 连接和定时器
   *
   * SSE 是浏览器原生资源，不清理会内存泄漏；重试定时器不清理会在
   * 卸载后仍然重建连接
   */
  const cleanup = useCallback(() => {
    Object.values(sseRefs.current).forEach(controller => controller.abort());
    Object.values(sseRetryTimers.current).forEach(timerId => clearTimeout(timerId));
    sseRefs.current = {}; // 清空连接注册表
    sseRetryCount.current = {}; // 清空重试计数器
    sseRetryTimers.current = {}; // 清空重试定时器注册表
  }, []);

  // 组件卸载时自动清理
  useEffect(() => {
    return cleanup;
  }, [cleanup]);

  // 返回公共接口
  return {
    listenToParseStatus,    // 开始监听论文解析状态
    reconnectPendingPapers, // 重连正在解析的论文（页面刷新后）
    cleanup,                // 手动清理所有连接
  };
}
