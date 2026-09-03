/**
 * 学术站点结构化提取器
 *
 * 为常见学术网站（arXiv、PubMed、Scholar）提供 DOM 级别的结构化数据提取
 */

import type { AcademicPaper } from './types'

/**
 * 根据 URL 域名自动路由到对应提取器
 */
export function routeToExtractor(url: string): ((html: string, url: string) => AcademicPaper) | null {
  const hostname = (() => {
    try {
      return new URL(url).hostname
    } catch {
      return ''
    }
  })()

  if (hostname.includes('arxiv.org')) return extractArxiv
  if (hostname.includes('ncbi.nlm.nih.gov') || hostname.includes('pubmed')) return extractPubmed
  if (hostname.includes('scholar.google')) return extractScholar

  return null
}

/**
 * 判断 URL 是否需要浏览器渲染
 */
export function needsBrowserRendering(url: string): boolean {
  try {
    const hostname = new URL(url).hostname
    return hostname.includes('scholar.google')
  } catch {
    return false
  }
}

/**
 * arXiv abs 页面提取
 */
export function extractArxiv(html: string, url: string): AcademicPaper {
  const parser = new DOMParser()
  const doc = parser.parseFromString(html, 'text/html')

  const title =
    doc.querySelector('h1.title')?.textContent?.trim() ||
    doc.querySelector('#page-header h1')?.textContent?.trim() ||
    ''

  const abstractText =
    doc.querySelector('blockquote.abstract')?.textContent?.trim() ||
    doc.querySelector('.abstract')?.textContent?.trim() ||
    ''

  const authors = Array.from(doc.querySelectorAll('.authors a')).map(
    (el) => el.textContent?.trim() || '',
  ).filter(Boolean)

  const categories =
    doc.querySelector('.subjects')?.textContent?.trim() ||
    doc.querySelector('.primary-subject')?.textContent?.trim() ||
    ''

  const arxivId = url.split('/').pop() || ''

  return {
    title,
    authors,
    abstract_text: cleanAbstract(abstractText),
    url,
    source: 'arxiv',
    extra: { arxiv_id: arxivId, categories },
  }
}

/**
 * PubMed 文章页面提取
 */
export function extractPubmed(html: string, url: string): AcademicPaper {
  const parser = new DOMParser()
  const doc = parser.parseFromString(html, 'text/html')

  const title =
    doc.querySelector('h1.heading-title')?.textContent?.trim() ||
    doc.querySelector('.article-title')?.textContent?.trim() ||
    ''

  const abstractText =
    doc.querySelector('div.abstract-content')?.textContent?.trim() ||
    doc.querySelector('.abstract')?.textContent?.trim() ||
    ''

  const authors = Array.from(
    doc.querySelectorAll('a.full-name, span.authors a, .contribs-group a'),
  ).map((el) => el.textContent?.trim() || '').filter(Boolean)

  const pmid =
    doc.querySelector('strong.current-id')?.getAttribute('data-article-pmid') ||
    doc.querySelector('.current-id')?.textContent?.trim() ||
    ''

  const doi =
    doc.querySelector('span.doi a')?.textContent?.trim() ||
    doc.querySelector("a[data-ga-action='doi']")?.textContent?.trim() ||
    ''

  const journal =
    doc.querySelector('button.journal-actions-trigger')?.textContent?.trim() ||
    doc.querySelector('.citation-title')?.textContent?.trim() ||
    ''

  const year = extractYear(doc)

  return {
    title,
    authors,
    abstract_text: cleanAbstract(abstractText),
    year,
    doi: doi || undefined,
    url,
    source: 'pubmed',
    extra: { pmid, journal },
  }
}

/**
 * Google Scholar 搜索结果提取
 */
export function extractScholar(html: string, url: string): AcademicPaper {
  const parser = new DOMParser()
  const doc = parser.parseFromString(html, 'text/html')

  const title =
    doc.querySelector('h3.gs_rt')?.textContent?.trim() || ''

  const snippet =
    doc.querySelector('div.gs_rs')?.textContent?.trim() || ''

  return {
    title,
    authors: [],
    abstract_text: snippet,
    url,
    source: 'scholar',
    extra: { partial: true },
  }
}

/**
 * 提取所有 Scholar 搜索结果
 */
export function extractScholarResults(html: string): AcademicPaper[] {
  const parser = new DOMParser()
  const doc = parser.parseFromString(html, 'text/html')

  return Array.from(doc.querySelectorAll('div.gs_r.gs_or.gs_scl')).map((el) => {
    const title = el.querySelector('h3.gs_rt')?.textContent?.trim() || ''
    const snippet = el.querySelector('div.gs_rs')?.textContent?.trim() || ''
    const link = el.querySelector('h3.gs_rt a')?.getAttribute('href') || ''

    return {
      title,
      authors: [],
      abstract_text: snippet,
      url: link,
      source: 'scholar',
      extra: { partial: true },
    }
  }).filter((p) => p.title.length > 0)
}

function cleanAbstract(text: string): string {
  return text
    .replace(/^Abstract[:\s]*/i, '')
    .replace(/\s+/g, ' ')
    .trim()
}

function extractYear(doc: Document): number | undefined {
  const text =
    doc.querySelector('meta[name="citation_date"]')?.getAttribute('content') ||
    doc.querySelector('span.date, .cit')?.textContent ||
    ''

  const match = text.match(/\b(19|20)\d{2}\b/)
  return match ? parseInt(match[0], 10) : undefined
}
