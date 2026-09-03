/**
 * bibliographyApi 测试（autonomics 导入管线契约）
 *
 * - importBibliography：读文件 → POST /articles/import/batch（format 由
 *   扩展名推导、category_id 透传）；响应映射回 jayread 的 ImportResult
 *   （failed 数组 → 计数、articles → safeId 列表、duplicate_details 恒空）
 * - attachPdf：multipart file → POST /articles/{enc}/fulltext
 * - resolveDuplicates / downloadPdf：显式拒绝
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  importBibliography,
  attachPdf,
  resolveDuplicates,
  downloadPdf,
} from '../bibliographyApi';
import { toSafeId, type BibArticle } from '../mapping';
import { invalidateCache } from '../client';

const REAL_ID = 'doi:10.1000/batch1';
const ENC = 'doi%3A10.1000%2Fbatch1';

function makeArticle(overrides: Partial<BibArticle> = {}): BibArticle {
  return {
    id: REAL_ID,
    title: 'Batch Import Fixture',
    authors: [{ last_name: 'Garcia', fore_name: 'Ben' }],
    identifiers: [{ kind: 'doi', value: '10.1000/batch1' }],
    abstract_text: null,
    year: 2025,
    month: null,
    journal: null,
    volume: null,
    issue: null,
    pages: null,
    issn: null,
    essn: null,
    language: null,
    pub_types: ['Journal Article'],
    keywords: [],
    source: 'bibtex',
    created_at: '2026-03-01T00:00:00Z',
    updated_at: '2026-03-01T00:00:00Z',
    ...overrides,
  };
}

function jsonResponse(body: unknown, ok = true, status = 200) {
  return {
    ok,
    status,
    statusText: ok ? 'OK' : 'Error',
    headers: { get: () => 'application/json' },
    json: () => Promise.resolve(body),
  };
}

function makeFetch(impl?: (...args: unknown[]) => unknown) {
  const f = vi.fn(impl ?? (() => Promise.resolve(jsonResponse({ ok: true }))));
  global.fetch = f as unknown as typeof fetch;
  return f;
}

function callOf(f: ReturnType<typeof makeFetch>, index = 0) {
  const [url, init] = f.mock.calls[index] as [string, RequestInit];
  return { url, init };
}

beforeEach(() => {
  vi.clearAllMocks();
  invalidateCache();
});

afterEach(() => {
  vi.restoreAllMocks();
  invalidateCache();
});

// ============================================================================
// importBibliography
// ============================================================================

describe('importBibliography — POST /articles/import/batch', () => {
  it('.bib → format=bibtex，content 为文件文本，category_id 透传；响应映射回 ImportResult', async () => {
    const batchBody = {
      imported: 2,
      duplicates: 1,
      failed: [{ key: 'bad1', reason: 'missing title' }],
      duplicate_details: [
        { title: 'Dup', identifiers: [], existing_id: 'doi:10.1/dup' },
      ],
      articles: [makeArticle(), makeArticle({ id: 'doi:10.1000/batch2' })],
    };
    const f = makeFetch(() => Promise.resolve(jsonResponse(batchBody)));

    const file = new File(['@article{a, title={A}}'], 'refs.bib', { type: 'text/plain' });
    const res = await importBibliography(file, 'col-3');

    const { url, init } = callOf(f);
    expect(url).toBe('/api/v1/bib/articles/import/batch');
    expect(init.method).toBe('POST');
    const body = JSON.parse(init.body as string) as Record<string, unknown>;
    expect(body.format).toBe('bibtex');
    expect(body.content).toBe('@article{a, title={A}}');
    expect(body.category_id).toBe('col-3');

    // 映射：failed 数组 → 计数；articles → safeId 列表；duplicate_details 恒空
    expect(res.imported).toBe(2);
    expect(res.duplicates).toBe(1);
    expect(res.failed).toBe(1);
    expect(res.duplicate_details).toEqual([]);
    expect(res.imported_paper_ids).toEqual([
      toSafeId('doi:10.1000/batch1'),
      toSafeId('doi:10.1000/batch2'),
    ]);
  });

  it('扩展名推导 format：.ris→ris、.json→csl_json、.txt→auto；无分类时不发 category_id', async () => {
    const cases: Array<[string, string]> = [
      ['refs.ris', 'ris'],
      ['refs.json', 'csl_json'],
      ['refs.txt', 'auto'],
    ];
    for (const [name, format] of cases) {
      const f = makeFetch(() =>
        Promise.resolve(jsonResponse({ imported: 0, duplicates: 0, failed: [], duplicate_details: [], articles: [] })),
      );
      await importBibliography(new File(['x'], name));
      const { init } = callOf(f);
      const body = JSON.parse(init.body as string) as Record<string, unknown>;
      expect(body.format).toBe(format);
      expect('category_id' in body).toBe(false);
    }
  });

  it('显式 format 提示优先于扩展名推导', async () => {
    const f = makeFetch(() =>
      Promise.resolve(jsonResponse({ imported: 0, duplicates: 0, failed: [], duplicate_details: [], articles: [] })),
    );
    await importBibliography(new File(['x'], 'refs.txt'), null, 'ris');
    const { init } = callOf(f);
    const body = JSON.parse(init.body as string) as Record<string, unknown>;
    expect(body.format).toBe('ris');
  });
});

// ============================================================================
// attachPdf
// ============================================================================

describe('attachPdf — POST /articles/{enc}/fulltext', () => {
  it('multipart file 上传到真实 ID 百分号编码的路径，返回 safeId', async () => {
    const f = makeFetch(() => Promise.resolve(jsonResponse({ fulltext: { article_id: REAL_ID } })));

    const res = await attachPdf(toSafeId(REAL_ID), new File(['%PDF-1.4'], 'paper.pdf'));

    const { url, init } = callOf(f);
    expect(url).toBe(`/api/v1/bib/articles/${ENC}/fulltext`);
    expect(init.method).toBe('POST');
    expect(init.body).toBeInstanceOf(FormData);
    expect((init.body as FormData).get('file')).toBeInstanceOf(File);
    expect(res.paper_id).toBe(toSafeId(REAL_ID));
  });
});

// ============================================================================
// 桩
// ============================================================================

describe('桩化函数', () => {
  it('resolveDuplicates reject（服务端已合并）', async () => {
    await expect(resolveDuplicates([])).rejects.toThrow(/服务端合并/);
  });

  it('downloadPdf reject（无开放获取管线）', async () => {
    await expect(downloadPdf(toSafeId(REAL_ID))).rejects.toThrow(/开放获取/);
  });
});
