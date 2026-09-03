/**
 * BrowserService 单例
 *
 * 监听来自 Rust 后端的浏览器任务，执行操作后回传结果。
 * 支持 Tauri 桌面模式（事件驱动）和 Web 模式（轮询）。
 */

import type { BrowserTask, BrowserResult, BrowserOp } from './types'
import { fetchPage, extractFromHtml, extractText } from './operations'
import { routeToExtractor, needsBrowserRendering } from './academicExtractors'

type ListenFn = (event: string, handler: (event: { payload: string }) => void) => void
type InvokeFn = (cmd: string, args: Record<string, unknown>) => Promise<unknown>

let service: BrowserService | null = null

export class BrowserService {
  private listen: ListenFn | null = null
  private invoke: InvokeFn | null = null
  private pollInterval: ReturnType<typeof setInterval> | null = null
  private baseUrl = ''

  private constructor() {}

  static getInstance(): BrowserService {
    if (!service) {
      service = new BrowserService()
    }
    return service
  }

  /**
   * 初始化 Tauri 模式
   */
  initTauri(listen: ListenFn, invoke: InvokeFn): void {
    this.listen = listen
    this.invoke = invoke

    listen('browser-task', (event) => {
      const task: BrowserTask = JSON.parse(event.payload)
      this.handleTask(task)
    })

    console.log('[BrowserService] Tauri 模式已初始化')
  }

  /**
   * 初始化 Web 模式（轮询后端获取待处理任务）
   */
  initWeb(baseUrl: string): void {
    this.baseUrl = baseUrl

    this.pollInterval = setInterval(() => {
      this.pollPendingTasks()
    }, 2000)

    console.log('[BrowserService] Web 模式已初始化')
  }

  /**
   * 停止服务
   */
  stop(): void {
    if (this.pollInterval) {
      clearInterval(this.pollInterval)
      this.pollInterval = null
    }
    service = null
  }

  private async handleTask(task: BrowserTask): Promise<void> {
    const start = performance.now()
    let result: BrowserResult

    try {
      const data = await this.executeOperation(task)
      result = {
        request_id: task.request_id,
        success: true,
        data: data as Record<string, unknown>,
        execution_time_ms: Math.round(performance.now() - start),
      }
    } catch (err) {
      result = {
        request_id: task.request_id,
        success: false,
        data: {},
        error: err instanceof Error ? err.message : String(err),
        execution_time_ms: Math.round(performance.now() - start),
      }
    }

    this.submitResult(result)
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private async executeOperation(task: BrowserTask): Promise<any> {
    const op = task.operation as BrowserOp

    switch (op) {
      case 'fetch_page': {
        const result = await fetchPage(task.url)
        // 尝试学术站点提取
        const extractor = routeToExtractor(task.url)
        if (extractor) {
          const paper = extractor(result.text, task.url)
          return { ...result, academic: paper }
        }
        return result
      }

      case 'extract_text': {
        const html = task.html || (await fetch(task.url).then((r) => r.text()))
        return { text: extractText(html!) }
      }

      case 'extract_metadata': {
        const html = task.html || (await fetch(task.url).then((r) => r.text()))
        const result = extractFromHtml(html!, task.url)
        return { metadata: result.metadata, title: result.title }
      }

      case 'extract_academic': {
        const extractor = routeToExtractor(task.url)
        if (!extractor) {
          throw new Error(`No academic extractor for: ${task.url}`)
        }
        const html = task.html || (await fetch(task.url).then((r) => r.text()))
        return extractor(html!, task.url)
      }

      case 'render_and_extract': {
        // 对于需要 JS 渲染的页面，尝试 fetch + DOMParser
        // 如果站点已知需要 JS，标记为需要 fallback
        if (needsBrowserRendering(task.url)) {
          throw new Error(
            '此页面需要完整的浏览器渲染，请使用 CDP 方式（Layer 3）',
          )
        }
        return fetchPage(task.url)
      }

      default:
        throw new Error(`未知操作: ${op}`)
    }
  }

  private async submitResult(result: BrowserResult): Promise<void> {
    if (this.invoke) {
      // Tauri 模式
      try {
        await this.invoke('submit_browser_result', { result })
      } catch (err) {
        console.error('[BrowserService] 提交结果失败:', err)
      }
    } else if (this.baseUrl) {
      // Web 模式
      try {
        await fetch(`${this.baseUrl}/api/browser/submit`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(result),
        })
      } catch (err) {
        console.error('[BrowserService] 提交结果失败:', err)
      }
    }
  }

  private async pollPendingTasks(): Promise<void> {
    if (!this.baseUrl) return

    try {
      const resp = await fetch(`${this.baseUrl}/api/browser/pending`)
      if (!resp.ok) return

      const tasks: BrowserTask[] = await resp.json()
      for (const task of tasks) {
        this.handleTask(task)
      }
    } catch {
      // 忽略轮询错误
    }
  }
}
