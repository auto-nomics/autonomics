/**
 * useSSEProgress - SSE 进度监听钩子
 *
 * 本文件封装了论文解析进度的 SSE（Server-Sent Events）监听逻辑，包括：
 * - 为指定论文建立 SSE 连接，监听解析进度
 * - 处理 progress、done、error 三种事件类型
 * - 指数退避重试策略：最多重试 3 次，延迟 1秒 → 2秒 → 4秒
 * - 组件卸载时自动清理连接和定时器
 *
 * 为什么使用 SSE 而不是轮询？
 * - SSE 允许服务器主动推送，延迟更低（< 100ms vs 2-5s）
 * - 减少无效请求，降低服务器负载
 * - 浏览器原生支持，自动重连机制
 *
 * 为什么需要手动管理 SSE 连接？
 * - React 没有内置的 SSE 钩子
 * - 需要在组件卸载时手动清理，防止内存泄漏
 * - 同一论文可能触发多次解析，需要关闭旧连接
 */

import { useRef, useCallback, useEffect } from 'react'; // 导入 React 核心钩子
import { App } from 'antd'; // 导入 Ant Design App 组件
import type { Paper } from '@/types';
import { getStreamUrl } from '../../../services/papersApi'; // 导入获取 SSE 端点 URL 的函数

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
 * @param {Function} onProgress - 进度更新回调，签名：({ percent: number, stage: string }) => void
 * @param {Function} onDone - 解析完成回调，签名：({ parseEngine: string, markdownLength: number }) => void
 * @param {Function} onError - 解析失败回调，签名：(errorMessage: string) => void
 * @returns {Object} 返回包含 listenToParseStatus 函数和清理函数的对象
 */
export function useSSEProgress({ onProgress, onDone, onError }: UseSSEProgressParams) {
  const { message } = App.useApp();
  // SSE 连接注册表：使用 ref 存储活跃的 SSE 连接（不触发重渲染）
  // 结构：{ paperId: EventSource }
  const sseRefs = useRef<Record<string, any>>({}); // ref：存储每篇论文的 SSE 连接实例

  // SSE 重试计数器：记录每个论文的重试次数（用于指数退避策略）
  // 结构：{ paperId: number }
  const sseRetryCount = useRef<Record<string, number>>({}); // ref：存储每篇论文的当前重试次数

  // SSE 重试定时器注册表：存储每个重试操作的 setTimeout ID（用于组件卸载时清理）
  // 结构：{ paperId: number }
  const sseRetryTimers = useRef<Record<string, any>>({}); // ref：存储每个重试操作的定时器 ID

  /**
   * 监听论文解析状态的 SSE 推送
   *
   * SSE 事件类型：
   * - progress: 解析进行中，定期推送进度
   * - done: 解析成功完成
   * - error: 解析失败
   *
   * @param {number} paperId - 需要监听的论文 ID
   */
  const listenToParseStatus = useCallback((paperId: string) => {
    // 如果该论文已有监听连接，先关闭（避免重复连接）
    // 这可能在用户快速点击重解析时发生
    if (sseRefs.current[paperId]) { // 检查是否已有该论文的 SSE 连接
      sseRefs.current[paperId].close(); // 关闭旧连接，防止重复
    }

    // 创建新的 SSE 连接，监听后端推送的解析进度
    const es = new EventSource(getStreamUrl(paperId)); // 创建 EventSource 实例，连接后端 SSE 端点
    // 将连接存储到 ref 中，便于后续管理和清理
    sseRefs.current[paperId] = es; // 将 EventSource 实例存储到 ref 注册表中

    /**
     * 监听解析进度事件
     * 后端在解析过程中会定期发送 progress 事件
     */
    es.addEventListener('progress', (e: any) => {
      // 解析后端发送的 JSON 数据，包含 percent（进度百分比，整数）和 stage（当前阶段描述，中文文字）
      // 后端心跳协程每 2 秒推送一次，percent 平滑递增，stage 随 MinerU CLI 输出更新
      try {
        const data = JSON.parse(e.data); // 解析事件数据为 JSON 对象，如 {"percent": 42, "stage": "MinerU: 识别数学公式..."}

        // 调用父组件传入的进度更新回调
        // 传递论文 ID、进度百分比和阶段描述
        onProgress?.({
          paperId,      // 论文 ID
          percent: data.percent ?? 0, // 进度百分比，null 时默认 0
          stage: data.stage || '',     // 阶段描述，空字符串默认
        });
      } catch (err) {
        console.warn('[SSE] Failed to parse progress event data:', e.data, err);
      }
    });

    /**
     * 监听解析完成事件
     * 后端解析完成后发送 done 事件，包含最终结果
     */
    es.addEventListener('done', (e: any) => {
      try {
        const data = JSON.parse(e.data); // 解析完成事件的 JSON 数据，包含 parse_engine（解析引擎名）和 markdown_length（Markdown 字符数）

        // 调用父组件传入的完成回调
        onDone?.({
          paperId,                    // 论文 ID
          parseEngine: data.parse_engine, // 解析引擎名称（mineru）
          markdownLength: data.markdown_length, // Markdown 字符数
        });

        // 显示成功提示
        message.success('论文解析完成！'); // 弹出 Ant Design 全局成功提示

        // 关闭 SSE 连接（已完成，无需继续监听）
        es.close(); // 关闭 EventSource 连接
        // 从 ref 中移除该连接引用
        delete sseRefs.current[paperId]; // 删除 ref 中对应的连接引用
        // 清除重试计数（解析成功，无需再重试）
        delete sseRetryCount.current[paperId]; // 删除该论文的重试计数
      } catch (err) {
        console.warn('[SSE] Failed to parse done event data:', e.data, err);
      }
    });

    /**
     * 监听解析失败事件
     * 后端解析失败时发送 error 事件
     */
    // 标记是否已处理后端发送的 error 事件（防止 onerror 不必要的重试）
    // 当后端主动发送 error 事件后关闭连接时，EventSource 的 onerror 也会触发
    // 通过此标记区分"合法解析错误"和"网络异常"，避免在合法错误后仍尝试重连
    let errorHandled = false; // 是否已处理后端的 error 事件（防止 onerror 重复重试）

    es.addEventListener('error', (e: any) => {
      // 只有当事件包含数据时才是真正的解析错误
      if (e.data) { // 检查事件是否携带数据
        errorHandled = true; // 标记已处理后端的 error 事件，防止 onerror 不必要的重试
        try {
          const data = JSON.parse(e.data); // 解析错误信息

          // 调用父组件传入的错误回调
          onError?.({
            paperId,        // 论文 ID
            message: data.message, // 错误信息
          });

          // 显示错误提示
          message.error(data.message || '解析失败'); // 弹出错误提示
        } catch (err) {
          console.warn('[SSE] Failed to parse error event data:', e.data, err);
        }
      }
      // 关闭连接并清理引用
      es.close(); // 关闭 EventSource 连接
      delete sseRefs.current[paperId]; // 删除 ref 中对应的连接引用
      // 清除重试计数（解析失败，无需再重试）
      delete sseRetryCount.current[paperId]; // 删除该论文的重试计数
    });

    /**
     * SSE 连接本身的错误处理
     * 当网络断开或服务器关闭连接时触发
     *
     * 重试策略：
     * - 使用指数退避算法，避免频繁重试加重服务器负担
     * - 最多重试 3 次，延迟时间：1秒 → 2秒 → 4秒
     * - 超过 3 次后放弃重试，等待用户手动触发（如点击"重解析"）
     */
    es.onerror = () => {
      // 如果已经处理后端的 error 事件（合法解析错误），跳过重试
      // 区分两种情况：
      // 1. 合法解析错误：后端发送 error 事件 → error handler 处理 → 连接关闭 → onerror 触发 → 不重试
      // 2. 网络异常：连接断开 → onerror 直接触发 → 需要重试
      if (errorHandled) return; // 已处理后端 error，跳过重试

      // 关闭异常连接
      es.close(); // 关闭连接
      // 清理引用
      delete sseRefs.current[paperId]; // 删除 ref 中对应的连接引用

      // 获取当前论文的重试次数，如果没有则初始化为 0
      const currentRetries = sseRetryCount.current[paperId] || 0; // 获取当前重试计数

      // 最多重试 3 次，超过则放弃
      if (currentRetries < 3) { // 检查是否达到最大重试次数
        // 更新重试计数（加 1）
        sseRetryCount.current[paperId] = currentRetries + 1; // 递增重试次数

        // 计算延迟时间：指数退避，1秒 → 2秒 → 4秒
        // Math.pow(2, n) 计算 2 的 n 次方：2^0=1, 2^1=2, 2^2=4
        const delay = Math.pow(2, currentRetries) * 1000; // 计算延迟毫秒数

        // 使用 setTimeout 延迟后重新调用 listenToParseStatus
        // 存储定时器 ID 到 sseRetryTimers，便于后续清理
        sseRetryTimers.current[paperId] = setTimeout(() => {
          // 延迟结束后重新尝试建立 SSE 连接
          listenToParseStatus(paperId); // 递归调用，重新建立 SSE 连接
        }, delay); // 使用计算出的延迟时间
      }
    };
  }, [onProgress, onDone, onError]); // 依赖项：回调函数

  /**
   * 初始 SSE 重连：页面加载/刷新后，为正在解析的论文自动重建 SSE 监听
   *
   * 为什么需要这个？
   * - 用户上传 PDF 后，前端通过 SSE 实时接收解析进度
   * - 如果用户在解析过程中刷新页面或关闭后重新打开，SSE 连接会被浏览器断开
   * - 但后端的解析任务是独立运行的后台协程，不依赖 SSE 连接，会继续执行
   * - 所以后端可能已经完成了解析，或者仍在继续解析
   * - 此时需要为 parse_status 为 'parsing' 或 'pending' 的论文重建 SSE 连接
   *
   * 覆盖的场景：
   * 1. 刷新页面 → 论文仍在解析 → 重建 SSE → 进度条恢复实时更新
   * 2. 关闭页面后重新打开 → 论文已解析完成 → loadPapers 返回 'done' → 不需要重连
   * 3. 解析到一半跳到另一篇论文再回来 → 组件未卸载（React Router） → SSE 连接仍在
   * 4. 后端重启导致解析中断 → parse_status 停留在 'parsing' → 重连后无事件 → 用户可手动重解析
   *
   * @param {Array} papers - 论文列表数组
   * @param {boolean} loading - 是否正在加载
   * @param {Function} hasReconnectedRef - 外部传入的重连标记 ref，用于控制只执行一次
   */
  const reconnectPendingPapers = useCallback((papers: Paper[], loading: boolean, hasReconnectedRef: React.MutableRefObject<boolean>) => {
    // loading 为 true 时表示论文列表尚未加载完成，跳过
    if (loading) return; // 等待 loadPapers 完成后再检查

    // hasReconnectedRef 确保重连逻辑只执行一次（避免 done 事件过渡期触发误重连）
    if (hasReconnectedRef.current) return; // 已执行过初始重连，跳过
    hasReconnectedRef.current = true; // 标记为已执行，后续不再触发

    // 遍历所有论文，为正在解析或等待解析的论文重建 SSE 连接
    papers.forEach(p => { // 遍历论文列表
      // 处理 parse_status 为 'parsing'（前端 SSE 临时状态）、'processing'（后端存储的解析中状态）或 'pending'（等待解析）的论文
      // 且检查 sseRefs 确认当前没有活跃的 SSE 连接（避免重复连接）
      if ((p.parse_status === 'parsing' || p.parse_status === 'processing' || p.parse_status === 'pending') && !sseRefs.current[p.id]) {
        listenToParseStatus(p.id); // 为该论文建立新的 SSE 连接，恢复进度推送
      }
    });
  }, [listenToParseStatus]); // 依赖项：SSE 监听函数

  /**
   * 清理所有 SSE 连接和定时器
   *
   * 为什么需要这个 cleanup？
   * - SSE 连接是浏览器原生资源，不会随组件卸载自动清理
   * - 不清理会导致内存泄漏和无效请求
   * - 多次挂载/卸载会创建多个连接，浪费资源
   */
  const cleanup = useCallback(() => {
    // 遍历所有活跃的 SSE 连接并关闭
    Object.values(sseRefs.current).forEach((es: any) => es.close()); // 逐一关闭所有活跃的 SSE 连接
    // 清理所有重试定时器，防止组件卸载后仍然执行重试
    Object.values(sseRetryTimers.current).forEach((timerId: any) => clearTimeout(timerId)); // 逐一清除所有重试定时器
    // 清空 ref 对象
    sseRefs.current = {}; // 清空连接注册表
    sseRetryCount.current = {}; // 清空重试计数器
    sseRetryTimers.current = {}; // 清空重试定时器注册表
  }, []);

  // 组件卸载时自动清理
  useEffect(() => {
    // 返回清理函数，在组件卸载时执行
    return cleanup; // 返回清理函数
  }, [cleanup]); // 依赖项：清理函数

  // 返回公共接口
  return {
    listenToParseStatus,    // 开始监听论文解析状态
    reconnectPendingPapers, // 重连正在解析的论文（页面刷新后）
    cleanup,                // 手动清理所有连接
  };
}

/**
 * 导出类型定义（用于 TypeScript 项目）
 * 本项目使用 JavaScript，保留类型注释供参考
 */
/**
 * @typedef {Object} ProgressData
 * @property {number} paperId - 论文 ID
 * @property {number} percent - 进度百分比（0-100）
 * @property {string} stage - 当前阶段描述
 */

/**
 * @typedef {Object} DoneData
 * @property {number} paperId - 论文 ID
 * @property {string} parseEngine - 解析引擎名称（mineru）
 * @property {number} markdownLength - Markdown 字符数
 */

/**
 * @typedef {Object} ErrorData
 * @property {number} paperId - 论文 ID
 * @property {string} message - 错误信息
 */
