/**
 * BrowserService 单例
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * jayread 用这个服务接 Rust 侧的浏览器任务队列（Tauri 事件驱动 / Web 轮询
 * 浏览器待办队列）抓取需要 JS 渲染的学术页面。autonomics 是纯浏览器
 * 部署，没有浏览器自动化后端。
 *
 * 这里保留类的公共形状（getInstance / initTauri / initWeb / stop），但
 * `initTauri` / `initWeb` 都变成 no-op —— 不挂事件监听、不起轮询定时器，
 * `handleTask` 因此永远不会被触发，网络提交与轮询代码一并移除。
 *
 * `initBrowserService()`（见 ./index.ts）是唯一入口，且 Phase 0 已从 main.tsx
 * 移除调用，本文件当前无任何运行时消费方；保留导出以维持签名，Phase 5 清扫。
 */

import type { BrowserTask, BrowserResult } from './types'

type ListenFn = (event: string, handler: (event: { payload: string }) => void) => void
type InvokeFn = (cmd: string, args: Record<string, unknown>) => Promise<unknown>

let service: BrowserService | null = null

export class BrowserService {
  private constructor() {}

  static getInstance(): BrowserService {
    if (!service) {
      service = new BrowserService()
    }
    return service
  }

  /**
   * 初始化 Tauri 模式
   *
   * ⚠️ 桩：autonomics 无 Tauri 壳、无浏览器任务队列。不挂任何监听。
   */
  initTauri(_listen: ListenFn, _invoke: InvokeFn): void {
    void _listen
    void _invoke
    console.log('[BrowserService] stubbed: autonomics 无浏览器任务后端，跳过初始化')
  }

  /**
   * 初始化 Web 模式（jayread 会在此起 2s 轮询）
   *
   * ⚠️ 桩：不起轮询，浏览器自动化端点不存在。
   */
  initWeb(_baseUrl: string): void {
    void _baseUrl
    console.log('[BrowserService] stubbed: autonomics 无浏览器任务后端，跳过初始化')
  }

  /**
   * 停止服务
   */
  stop(): void {
    service = null
  }

  /**
   * 处理单个浏览器任务
   *
   * ⚠️ 桩：没有任务来源；即便被直接调用也返回失败结果而不是发起网络请求。
   */
  private async handleTask(task: BrowserTask): Promise<BrowserResult> {
    return {
      request_id: task.request_id,
      success: false,
      data: {},
      error: '浏览器自动化暂不可用（autonomics 无浏览器任务后端）',
      execution_time_ms: 0,
    }
  }
}
