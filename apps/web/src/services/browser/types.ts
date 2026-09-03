/**
 * 浏览器桥接类型定义
 *
 * 与 Rust 端 frontend_bridge::types 保持一致
 */

export type BrowserOp =
  | 'fetch_page'
  | 'extract_text'
  | 'extract_metadata'
  | 'extract_academic'
  | 'render_and_extract'

export interface BrowserTask {
  request_id: string
  operation: BrowserOp
  url: string
  html?: string
  options: Record<string, unknown>
  timeout_ms: number
}

export interface BrowserResult {
  request_id: string
  success: boolean
  data: Record<string, unknown>
  error?: string
  execution_time_ms: number
}

export interface FetchPageResult {
  url: string
  status: number
  title: string
  text: string
  metadata: Record<string, unknown>
  links: Array<{ href: string; text: string }>
  content_length: number
}

export interface AcademicPaper {
  title: string
  authors: string[]
  abstract_text: string
  year?: number
  doi?: string
  url: string
  source: string
  extra: Record<string, unknown>
}
