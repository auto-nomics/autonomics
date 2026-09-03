/**
 * highlightsApi 测试（autonomics annotations 契约）
 *
 * 高亮映射：annotation.kind='highlight'、content=选中文字、page=1 起页码、
 * data={"rects":[...],"color":"#...","note":"..."}
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  fetchHighlights,
  createHighlight,
  updateHighlight,
  deleteHighlight,
  type BibAnnotation,
} from '../highlightsApi';
import { toSafeId } from '../mapping';
import { invalidateCache } from '../client';

// Annotation 本体没有 article_id 字段（归属靠挂载路径），这里只需要 safeId
const SAFE_ID = toSafeId('doi:10.1000/xyz');
const ENC = 'doi%3A10.1000%2Fxyz';

function makeAnnotation(overrides: Partial<BibAnnotation> = {}): BibAnnotation {
  return {
    id: 'ann-1',
    kind: 'highlight',
    content: 'selected text',
    page: 3,
    data: { rects: [{ x: 10, y: 20, w: 100, h: 12 }], color: '#ffeb3b' },
    created_at: '2026-03-01T10:00:00Z',
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
  return { url, init, body: typeof init.body === 'string' ? JSON.parse(init.body) : init.body };
}

beforeEach(() => {
  vi.clearAllMocks();
  invalidateCache();
});

afterEach(() => {
  vi.restoreAllMocks();
  invalidateCache();
});

describe('fetchHighlights', () => {
  it('GET /articles/{enc}/annotations（真实 ID 百分号编码）', async () => {
    const f = makeFetch(() => Promise.resolve(jsonResponse({ annotations: [] })));

    await fetchHighlights(SAFE_ID);

    expect(callOf(f).url).toBe('/api/v1/bib/articles/' + ENC + '/annotations');
  });

  it('带 page 过滤时追加 ?page=N（1 起）', async () => {
    const f = makeFetch(() => Promise.resolve(jsonResponse({ annotations: [] })));

    await fetchHighlights(SAFE_ID, 4);

    expect(callOf(f).url).toBe('/api/v1/bib/articles/' + ENC + '/annotations?page=4');
  });

  it('annotation → Highlight：rects/color 从 data 展开，text 来自 content', async () => {
    makeFetch(() => Promise.resolve(jsonResponse({ annotations: [makeAnnotation()] })));

    const { highlights, total } = await fetchHighlights(SAFE_ID);

    expect(total).toBe(1);
    expect(highlights[0]).toMatchObject({
      id: 'ann-1',
      paper_id: SAFE_ID,
      page: 3,
      text: 'selected text',
      color: '#ffeb3b',
      rects: [{ x: 10, y: 20, w: 100, h: 12 }],
      created_at: '2026-03-01T10:00:00Z',
    });
  });

  it('data 缺失时给中性默认值（color/rects 不为 undefined，渲染层不崩）', async () => {
    makeFetch(() =>
      Promise.resolve(jsonResponse({
        annotations: [makeAnnotation({ data: null, page: null })],
      }))
    );

    const { highlights } = await fetchHighlights(SAFE_ID);
    expect(highlights[0].color).toBe('#ffeb3b');
    expect(highlights[0].rects).toEqual([]);
    expect(highlights[0].page).toBe(1);
  });

  it('响应缺 annotations 时返回空列表', async () => {
    makeFetch(() => Promise.resolve(jsonResponse({})));

    const res = await fetchHighlights(SAFE_ID);
    expect(res).toEqual({ highlights: [], total: 0 });
  });

  it('只返回 kind=highlight 由调用方/后端负责 —— 本层不做过滤，但 note/comment 也能安全映射', async () => {
    makeFetch(() =>
      Promise.resolve(jsonResponse({
        annotations: [makeAnnotation({ kind: 'note', data: null })],
      }))
    );

    const { highlights } = await fetchHighlights(SAFE_ID);
    expect(highlights[0].text).toBe('selected text');
    expect(highlights[0].rects).toEqual([]);
  });
});

describe('createHighlight', () => {
  it('POST body 映射为 annotation 形状（kind/content/page/data.rects/data.color）', async () => {
    const f = makeFetch(() => Promise.resolve(jsonResponse({ annotation: makeAnnotation() })));

    await createHighlight(SAFE_ID, {
      page: 7,
      text: 'a quoted sentence',
      rects: [{ x: 1, y: 2, w: 3, h: 4 }],
      color: '#4ade80',
    });

    const { url, init, body } = callOf(f);
    expect(url).toBe('/api/v1/bib/articles/' + ENC + '/annotations');
    expect(init.method).toBe('POST');
    expect(body).toEqual({
      kind: 'highlight',
      content: 'a quoted sentence',
      page: 7,
      data: { rects: [{ x: 1, y: 2, w: 3, h: 4 }], color: '#4ade80' },
    });
  });

  it('未指定 color 时补默认黄色', async () => {
    const f = makeFetch(() => Promise.resolve(jsonResponse({ annotation: makeAnnotation() })));

    await createHighlight(SAFE_ID, { page: 1, text: 't', rects: [] });

    expect(callOf(f).body.data.color).toBe('#ffeb3b');
  });

  it('返回的 Highlight.paper_id 是调用方持有的 safeId', async () => {
    makeFetch(() => Promise.resolve(jsonResponse({ annotation: makeAnnotation() })));

    const hl = await createHighlight(SAFE_ID, { page: 1, text: 't', rects: [] });
    expect(hl.paper_id).toBe(SAFE_ID);
  });
});

describe('updateHighlight', () => {
  it('先读现值合并 data 再 PUT 到 /annotations/{id}（data 是覆盖语义）', async () => {
    const f = makeFetch((url: unknown) => {
      const u = String(url);
      if (u.endsWith('/annotations')) {
        // 第一步：读现值
        return Promise.resolve(jsonResponse({ annotations: [makeAnnotation()] }));
      }
      // 第二步：PUT → {"annotation": {...}}
      return Promise.resolve(jsonResponse({ annotation: makeAnnotation() }));
    });

    await updateHighlight(SAFE_ID, 'ann-1', { color: '#60a5fa' });

    // 第 2 个请求是 PUT /annotations/ann-1
    const { url, init, body } = callOf(f, 1);
    expect(url).toBe('/api/v1/bib/annotations/ann-1');
    expect(init.method).toBe('PUT');
    // rects 必须被保留下来，不能被覆盖成 undefined
    expect(body.data).toEqual({
      rects: [{ x: 10, y: 20, w: 100, h: 12 }],
      color: '#60a5fa',
    });
  });

  it('PUT 响应缺 annotation 时用本地合并结果兜底，返回形状不变', async () => {
    makeFetch((url: unknown) => {
      const u = String(url);
      if (u.endsWith('/annotations')) {
        return Promise.resolve(jsonResponse({ annotations: [makeAnnotation()] }));
      }
      return Promise.resolve(jsonResponse(null));
    });

    const hl = await updateHighlight(SAFE_ID, 'ann-1', { color: '#f472b6' });
    expect(hl.id).toBe('ann-1');
    expect(hl.color).toBe('#f472b6');
    expect(hl.rects).toEqual([{ x: 10, y: 20, w: 100, h: 12 }]);
  });
});

describe('deleteHighlight', () => {
  it('DELETE /annotations/{id}（端点挂在全局命名空间，不需要论文 id）', async () => {
    const f = makeFetch(() => Promise.resolve(jsonResponse(null)));

    await deleteHighlight(SAFE_ID, 'ann-9');

    const { url, init } = callOf(f);
    expect(url).toBe('/api/v1/bib/annotations/ann-9');
    expect(init.method).toBe('DELETE');
  });
});
