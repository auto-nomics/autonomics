/**
 * categoriesApi 测试（autonomics collections 契约）
 *
 * 覆盖：
 * - 扁平 collections → 前端推导 paper_count / total_count / uncategorized_count
 * - create / rename / delete 的端点与 body
 * - assign / unassign 的循环展开（后端只有单篇端点）
 * - moveCategory 的兄弟重排（sort_order 是兄弟下标，后端不做重排）
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  getCategoryTree,
  createCategory,
  renameCategory,
  deleteCategory,
  assignPapers,
  unassignPapers,
  moveCategory,
} from '../categoriesApi';
import { invalidateCache } from '../client';

function makeCollection(overrides: Record<string, unknown> = {}) {
  return {
    id: 'col-1',
    name: 'Root',
    description: null,
    tags: [],
    parent_id: null,
    sort_order: 0,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
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

/**
 * 简易路由 mock：根据 method + path 决定返回什么。
 * route 可以是 (method, path) => body | undefined
 */
function makeRouter(route: (method: string, path: string, body: unknown) => unknown) {
  const f = vi.fn((url: string, init?: RequestInit) => {
    const u = String(url).replace(/^\/api\/v1\/bib/, '');
    const method = (init?.method || 'GET').toUpperCase();
    const body = init?.body ? JSON.parse(init.body as string) : undefined;
    const result = route(method, u, body);
    return Promise.resolve(jsonResponse(result ?? {}));
  });
  global.fetch = f as unknown as typeof fetch;
  return f;
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
// getCategoryTree
// ============================================================================

describe('getCategoryTree — 扁平列表 + 计数推导', () => {
  it('GET /collections 与 GET /articles（用于计数）并行', async () => {
    const f = makeRouter((method, path) => {
      if (path === '/collections' && method === 'GET') return { collections: [] };
      return { total: 0, articles: [] };
    });

    const res = await getCategoryTree();

    expect(res.categories).toEqual([]);
    const paths = f.mock.calls.map((c) => String(c[0]).replace(/^\/api\/v1\/bib/, ''));
    expect(paths.some((p) => p.startsWith('/collections'))).toBe(true);
    expect(paths.some((p) => p.startsWith('/articles?'))).toBe(true);
  });

  it('paper_count / total_count / uncategorized_count 由 /collections 的 article_ids 推导', async () => {
    makeRouter((_m, path) => {
      if (path === '/collections') {
        return {
          collections: [
            makeCollection({ id: 'col-a', name: 'A', article_ids: ['doi:1', 'doi:2', 'doi:3'] }),
            makeCollection({ id: 'col-b', name: 'B', article_ids: ['doi:3'] }),
          ],
        };
      }
      // 只为拿 total；/articles?limit=1 的那一页内容不需要
      return { total: 4, articles: [], offset: 0, limit: 1 };
    });

    const res = await getCategoryTree();

    expect(res.total_count).toBe(4);
    // 去重后的已归档文章是 doi:1/2/3 → 4 - 3 = 1（跨集合计数不互斥，去重）
    expect(res.uncategorized_count).toBe(1);
    expect(res.categories).toEqual([
      expect.objectContaining({ id: 'col-a', name: 'A', paper_count: 3 }),
      expect.objectContaining({ id: 'col-b', name: 'B', paper_count: 1 }),
    ]);
  });

  it('文章计数去重：同一篇文章出现在多个集合只算一次已归档', async () => {
    makeRouter((_m, path) => {
      if (path === '/collections') {
        return {
          collections: [
            makeCollection({ id: 'col-a', article_ids: ['doi:1'] }),
            makeCollection({ id: 'col-b', article_ids: ['doi:1'] }),
          ],
        };
      }
      return { total: 3, articles: [] };
    });

    const res = await getCategoryTree();
    expect(res.uncategorized_count).toBe(2);
  });

  it('collections 按 sort_order 排序（前端建树依赖稳定顺序）', async () => {
    makeRouter((_m, path) => {
      if (path === '/collections') {
        return {
          collections: [
            makeCollection({ id: 'b', name: 'B', sort_order: 2 }),
            makeCollection({ id: 'a', name: 'A', sort_order: 1 }),
          ],
        };
      }
      return { total: 0, articles: [] };
    });

    const res = await getCategoryTree();
    expect(res.categories.map((c) => c.id)).toEqual(['a', 'b']);
  });

  it('Category 携带 parent_id / sort_order / created_at（useCategoryTree 的 raw 用到）', async () => {
    makeRouter((_m, path) => {
      if (path === '/collections') {
        return {
          collections: [makeCollection({ id: 'child', parent_id: 'p1', sort_order: 3 })],
        };
      }
      return { total: 0, articles: [] };
    });

    const res = await getCategoryTree();
    expect(res.categories[0]).toMatchObject({
      id: 'child',
      parent_id: 'p1',
      sort_order: 3,
      created_at: '2026-01-01T00:00:00Z',
      paper_count: 0,
    });
  });
});

// ============================================================================
// create / rename / delete
// ============================================================================

describe('createCategory', () => {
  it('POST /collections，带 name + parent_id + sort_order（排到兄弟末尾）', async () => {
    const f = makeRouter((method, path) => {
      if (path === '/collections' && method === 'GET') {
        return {
          collections: [
            makeCollection({ id: 's1', parent_id: 'p1', sort_order: 0 }),
            makeCollection({ id: 's2', parent_id: 'p1', sort_order: 1 }),
            makeCollection({ id: 'other', parent_id: null, sort_order: 0 }),
          ],
        };
      }
      // 后端返回 {"collection": {...}}
      return { collection: makeCollection({ id: 'new', name: 'Child', parent_id: 'p1', sort_order: 2 }) };
    });

    await createCategory('Child', 'p1');

    const postCall = f.mock.calls.find((c) => (c[1] as RequestInit)?.method === 'POST');
    const [, init] = postCall as [string, RequestInit];
    expect(String(postCall?.[0])).toBe('/api/v1/bib/collections');
    expect(JSON.parse(init.body as string)).toEqual({ name: 'Child', parent_id: 'p1', sort_order: 2 });
  });

  it('根级分类（parentId=null）不带 parent_id 字段', async () => {
    const f = makeRouter((method, path) => {
      if (path === '/collections' && method === 'GET') return { collections: [] };
      return { collection: makeCollection({ id: 'root', name: 'Root' }) };
    });

    await createCategory('Root', null);

    const postCall = f.mock.calls.find((c) => (c[1] as RequestInit)?.method === 'POST');
    expect(JSON.parse((postCall?.[1] as RequestInit).body as string)).toEqual({
      name: 'Root',
      sort_order: 0,
    });
  });
});

describe('renameCategory', () => {
  it('PUT /collections/{id}，body 只带 name', async () => {
    const f = makeRouter(() => ({ collection: makeCollection({ id: 'c1', name: 'Renamed' }) }));

    await renameCategory('c1', 'Renamed');

    const [url, init] = f.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/v1/bib/collections/c1');
    expect(init.method).toBe('PUT');
    expect(JSON.parse(init.body as string)).toEqual({ name: 'Renamed' });
  });
});

describe('deleteCategory', () => {
  it('DELETE /collections/{id}，返回 {message}', async () => {
    const f = makeRouter(() => null);

    const res = await deleteCategory('c1');

    const [url, init] = f.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/v1/bib/collections/c1');
    expect(init.method).toBe('DELETE');
    expect(res).toEqual({ message: '已删除' });
  });
});

// ============================================================================
// assign / unassign
// ============================================================================

describe('assignPapers / unassignPapers', () => {
  it('assign 展开为 paperIds × categoryIds 次单篇 POST（body 是真实 article_id）', async () => {
    const f = makeRouter(() => ({}));

    await assignPapers(['doi%3A10.1%2Fa', 'doi%3A10.1%2Fb'].map(decodeURIComponentSafe), ['col-1', 'col-2']);

    const posts = f.mock.calls.filter((c) => (c[1] as RequestInit)?.method === 'POST');
    expect(posts).toHaveLength(4); // 2 篇 × 2 集合

    const first = posts[0] as [string, RequestInit];
    expect(first[0]).toBe('/api/v1/bib/collections/col-1/articles');
    expect(JSON.parse(first[1].body as string)).toEqual({ article_id: 'doi:10.1/a' });

    // 4 次调用覆盖了所有组合
    const combos = posts.map(
      (c) => `${String(c[0]).split('/articles')[0].split('/collections/')[1]}|${JSON.parse((c[1] as RequestInit).body as string).article_id}`,
    );
    expect(combos.sort()).toEqual(['col-1|doi:10.1/a', 'col-1|doi:10.1/b', 'col-2|doi:10.1/a', 'col-2|doi:10.1/b']);
  });

  it('unassign 展开为 DELETE /collections/{id}/articles/{enc}', async () => {
    const f = makeRouter(() => null);

    await unassignPapers(['doi:10.1/a'], ['col-1']);

    const [url, init] = f.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/v1/bib/collections/col-1/articles/doi%3A10.1%2Fa');
    expect(init.method).toBe('DELETE');
  });
});

/** 测试辅助：把测试里写错的编码串还原成真实 ID */
function decodeURIComponentSafe(s: string): string {
  try { return decodeURIComponent(s); } catch { return s; }
}

// ============================================================================
// moveCategory
// ============================================================================

describe('moveCategory — 兄弟重排', () => {
  it('换父 + 落位，并把新父下的兄弟重排为 0..n-1', async () => {
    const f = makeRouter((method, path) => {
      if (path === '/collections' && method === 'GET') {
        return {
          collections: [
            makeCollection({ id: 'moving', name: 'Moving', parent_id: 'old', sort_order: 0 }),
            makeCollection({ id: 's0', name: 'S0', parent_id: 'newp', sort_order: 0 }),
            makeCollection({ id: 's1', name: 'S1', parent_id: 'newp', sort_order: 1 }),
            makeCollection({ id: 's2', name: 'S2', parent_id: 'newp', sort_order: 2 }),
          ],
        };
      }
      return makeCollection({ id: path.split('/').pop(), sort_order: 0 });
    });

    // 把 moving 插到 newp 兄弟列表的下标 1 → 期望最终顺序 s0(0) moving(1) s1(2) s2(3)
    await moveCategory('moving', 'newp', 1);

    const puts = f.mock.calls
      .filter((c) => (c[1] as RequestInit)?.method === 'PUT')
      .map((c) => {
        const id = String(c[0]).split('/collections/')[1];
        const body = JSON.parse((c[1] as RequestInit).body as string) as Record<string, unknown>;
        return { id, ...body } as { id: string; sort_order?: unknown; parent_id?: unknown };
      });

    // 第一次 PUT 是换父 + 落位
    expect(puts[0]).toEqual({ id: 'moving', parent_id: 'newp', sort_order: 1 });

    const putIds = puts.map((p) => p.id);
    // s0 的 sort_order 已经是 0，无需重写（跳过未变化的兄弟）
    expect(putIds).not.toContain('s0');
    // 插入点之后的兄弟要往后挪一位
    const orderOf = Object.fromEntries(puts.map((p) => [p.id, p.sort_order]));
    expect(orderOf.moving).toBe(1);
    expect(orderOf.s1).toBe(2);
    expect(orderOf.s2).toBe(3);
  });

  it('已处于目标下标的兄弟不会被重复 PUT', async () => {
    const f = makeRouter((method, path) => {
      if (path === '/collections' && method === 'GET') {
        return {
          collections: [
            makeCollection({ id: 'm', name: 'M', parent_id: 'old' }),
            makeCollection({ id: 's0', name: 'S0', parent_id: 'newp', sort_order: 0 }),
            makeCollection({ id: 's1', name: 'S1', parent_id: 'newp', sort_order: 1 }),
          ],
        };
      }
      return makeCollection({ id: 'm' });
    });

    // 插到末尾（下标 2）：s0/s1 位置不变，理论上只有换父那一次 PUT
    await moveCategory('m', 'newp', 2);

    const puts = f.mock.calls.filter((c) => (c[1] as RequestInit)?.method === 'PUT');
    expect(puts).toHaveLength(1);
  });

  it('移动到根级时 parent_id 显式置 null', async () => {
    const f = makeRouter((method, path) => {
      if (path === '/collections' && method === 'GET') {
        return { collections: [makeCollection({ id: 'm', name: 'M', parent_id: 'p1' })] };
      }
      return makeCollection({ id: 'm' });
    });

    await moveCategory('m', null, 0);

    const put = f.mock.calls.find((c) => (c[1] as RequestInit)?.method === 'PUT');
    const body = JSON.parse((put?.[1] as RequestInit).body as string);
    expect(body).toEqual({ parent_id: null, sort_order: 0 });
  });

  it('sortOrder 超出兄弟数量时收敛到末尾（不产生空洞）', async () => {
    const f = makeRouter((method, path) => {
      if (path === '/collections' && method === 'GET') {
        return {
          collections: [
            makeCollection({ id: 'm', name: 'M', parent_id: 'old' }),
            makeCollection({ id: 's0', name: 'S0', parent_id: 'newp', sort_order: 0 }),
          ],
        };
      }
      return makeCollection({ id: 'm' });
    });

    await moveCategory('m', 'newp', 99);

    const put = f.mock.calls.find((c) => (c[1] as RequestInit)?.method === 'PUT');
    const body = JSON.parse((put?.[1] as RequestInit).body as string);
    expect(body.sort_order).toBe(1); // 1 个兄弟 + 自己 → 末尾下标 1
  });

  it('返回 MoveCategoryResponse 形状（Category）', async () => {
    makeRouter((method, path) => {
      if (path === '/collections' && method === 'GET') {
        return { collections: [makeCollection({ id: 'm', name: 'M', parent_id: 'old' })] };
      }
      return makeCollection({ id: 'm' });
    });

    const res = await moveCategory('m', 'newp', 0);
    expect(res).toMatchObject({ id: 'm', name: 'M', parent_id: 'newp', sort_order: 0 });
    expect(typeof res.paper_count).toBe('number');
  });
});
