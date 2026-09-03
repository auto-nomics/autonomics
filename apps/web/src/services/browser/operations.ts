/**
 * 浏览器基础操作
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * 利用 WebView 内置的 fetch + DOMParser 执行网页抓取和内容提取。
 *
 * `fetchPage` 是唯一发起网络请求的导出（autonomics 无浏览器抓取后端，桩化）；
 * `extractFromHtml` / `extractText` 是纯本地 DOM 解析，保留原样 —— 不依赖任何
 * 后端能力，若 Phase 4 的 agentik 聊天工具需要解析 HTML 仍可复用。
 */

import type { FetchPageResult } from './types'

const NOISE_TAGS = ['SCRIPT', 'STYLE', 'NAV', 'FOOTER', 'HEADER', 'NOSCRIPT', 'SVG']

/**
 * 抓取页面并提取结构化内容
 *
 * ⚠️ 桩：autonomics 无浏览器抓取后端，不允许从前端任意抓取第三方页面。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function fetchPage(url: string): Promise<FetchPageResult> {
  throw new Error(`网页抓取暂不可用（autonomics 无浏览器抓取后端）：${url}`)
}

/**
 * 从 HTML 字符串提取结构化内容
 */
export function extractFromHtml(
  html: string,
  url: string,
  status: number = 200,
): FetchPageResult {
  const parser = new DOMParser()
  const doc = parser.parseFromString(html, 'text/html')

  const title = extractTitle(doc)
  const text = extractCleanText(doc)
  const metadata = extractMetadata(doc)
  const links = extractLinks(doc, url)

  return {
    url,
    status,
    title,
    text,
    metadata,
    links,
    content_length: html.length,
  }
}

/**
 * 提取纯文本（去除噪音元素）
 */
export function extractText(html: string): string {
  const parser = new DOMParser()
  const doc = parser.parseFromString(html, 'text/html')
  return extractCleanText(doc)
}

function extractTitle(doc: Document): string {
  const title = doc.querySelector('title')?.textContent?.trim()
  if (title) return title

  const h1 = doc.querySelector('h1')?.textContent?.trim()
  if (h1) return h1

  return ''
}

function extractCleanText(doc: Document): string {
  const body = doc.body
  if (!body) return ''

  // 克隆以避免修改原始 DOM
  const clone = body.cloneNode(true) as HTMLElement

  // 移除噪音元素
  for (const tag of NOISE_TAGS) {
    clone.querySelectorAll(tag).forEach((el) => el.remove())
  }

  const text = clone.textContent || ''
  return text.replace(/\s+/g, ' ').trim()
}

function extractMetadata(doc: Document): Record<string, unknown> {
  const meta: Record<string, unknown> = {}

  // meta description
  const desc = doc.querySelector("meta[name='description']")?.getAttribute('content')
  if (desc) meta.description = desc

  // Open Graph
  const ogEntries: Record<string, string> = {}
  doc.querySelectorAll("meta[property^='og:']").forEach((el) => {
    const prop = el.getAttribute('property')?.replace('og:', '')
    const content = el.getAttribute('content')
    if (prop && content) ogEntries[prop] = content
  })
  if (Object.keys(ogEntries).length > 0) meta.og = ogEntries

  // JSON-LD
  const jsonLd: unknown[] = []
  doc.querySelectorAll("script[type='application/ld+json']").forEach((el) => {
    try {
      jsonLd.push(JSON.parse(el.textContent || ''))
    } catch {
      // 忽略解析失败的 JSON-LD
    }
  })
  if (jsonLd.length > 0) meta.json_ld = jsonLd

  return meta
}

function extractLinks(
  doc: Document,
  baseUrl: string,
): Array<{ href: string; text: string }> {
  const links: Array<{ href: string; text: string }> = []
  const base = new URL(baseUrl)

  doc.querySelectorAll('a[href]').forEach((el) => {
    const href = el.getAttribute('href')
    if (!href) return

    try {
      const resolved = new URL(href, base).href
      const text = el.textContent?.trim()
      if (text && text.length > 0) {
        links.push({ href: resolved, text })
      }
    } catch {
      // 忽略无效 URL
    }
  })

  return links
}
