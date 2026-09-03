/**
 * BrowserService 初始化入口
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * jayread 在 main.tsx 启动时调用 initBrowserService() 激活浏览器抓取能力。
 * autonomics 是纯浏览器部署、无浏览器自动化后端，Phase 0 已移除 main.tsx 的
 * 调用；这里退化为 no-op，万一被调用也不会挂监听或起轮询。
 *
 * academicExtractors.ts 里的站点解析器是纯 HTML 解析（无网络、不发请求），
 * 保留原样 —— 若 Phase 4 的 agentik 聊天工具需要解析 HTML，它们仍然可用。
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
 * ⚠️ 桩：autonomics 无浏览器自动化后端，no-op。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function initBrowserService(): Promise<void> {
  console.log('[BrowserService] stubbed: autonomics 无浏览器任务后端，跳过初始化')
}
