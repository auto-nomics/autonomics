/**
 * BrowserService 初始化入口
 *
 * 在应用启动时调用 initBrowserService() 激活浏览器操作能力。
 */

export { BrowserService } from './BrowserService'
export type {
  BrowserOp,
  BrowserTask,
  BrowserResult,
  FetchPageResult,
  AcademicPaper,
} from './types'
export { fetchPage, extractText, extractFromHtml } from './operations'
export { routeToExtractor, needsBrowserRendering } from './academicExtractors'

/**
 * 初始化 BrowserService
 *
 * 自动检测运行模式（Tauri / Web）并选择对应的初始化方式。
 */
export async function initBrowserService(): Promise<void> {
  const { BrowserService } = await import('./BrowserService')
  const svc = BrowserService.getInstance()

  const isTauri = '__TAURI_INTERNALS__' in window

  if (isTauri) {
    try {
      const { listen } = await import('@tauri-apps/api/event')
      const { invoke } = await import('@tauri-apps/api/core')

      svc.initTauri(
        (event, handler) => {
          listen<string>(event, (e) => handler({ payload: e.payload }))
        },
        (cmd, args) => invoke(cmd, args),
      )
    } catch (err) {
      console.warn('[BrowserService] Tauri API 不可用，跳过初始化:', err)
    }
  } else {
    // Web 模式：使用当前页面 origin 作为后端 API 地址
    svc.initWeb(window.location.origin)
  }
}
