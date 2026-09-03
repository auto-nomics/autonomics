/**
 * API 客户端测试文件 (client.ts)
 *
 * 测试核心请求包装器，处理以下功能：
 * - URL 构造（带 /api/v1/bib 前缀）
 * - JSON 序列化
 * - 错误信封解析（autonomics 返回 {"error": "..."}）
 * - 超时与网络错误重试
 * - 响应缓存 + TTL 分类 + invalidateCache
 * - bearer token（localStorage.autonomics_token）
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import request, {
  DEFAULT_TIMEOUT,
  DEFAULT_RETRIES,
  API_BASE,
  invalidateCache,
  isTauri,
  isDesktop,
  getTauriBaseUrl,
} from '../client';

/** 组一个 autonomics 形状的 fetch Response mock */
function jsonResponse(body: unknown, ok = true, status = 200) {
  return {
    ok,
    status,
    statusText: status === 200 ? 'OK' : 'Error',
    headers: { get: () => 'application/json' },
    json: () => Promise.resolve(body),
  };
}

function makeFetch(impl?: (...args: unknown[]) => unknown) {
  return vi.fn(impl ?? (() => Promise.resolve(jsonResponse({ ok: true }))));
}

/**
 * happy-dom 在本测试环境里暴露的 localStorage 没有 getItem/setItem 方法
 * （见 useChatSender.test.ts 里同样的 "localStorage.getItem is not a function"
 * 失败），用内存 shim 替换，bearer-token 分支才能真正被验证。
 */
const tokenStore = new Map<string, string>();
const localStorageShim = {
  getItem: (k: string) => (tokenStore.has(k) ? (tokenStore.get(k) as string) : null),
  setItem: (k: string, v: string) => { tokenStore.set(k, String(v)); },
  removeItem: (k: string) => { tokenStore.delete(k); },
  clear: () => tokenStore.clear(),
};

beforeEach(() => {
  vi.clearAllMocks();
  invalidateCache();
  globalThis.localStorage = localStorageShim as unknown as Storage;
  tokenStore.clear();
});

afterEach(() => {
  vi.restoreAllMocks();
  tokenStore.clear();
});

describe('API Client - request()', () => {
  describe('Request Construction', () => {
    it('should prepend /api/v1/bib to the path', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/articles');

      expect(mockFetch).toHaveBeenCalledWith(
        '/api/v1/bib/articles',
        expect.objectContaining({})
      );
    });

    it('should use the documented API_BASE constant', () => {
      expect(API_BASE).toBe('/api/v1/bib');
    });

    it('should keep query strings intact', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/articles?limit=50&sort=created_at');

      expect(mockFetch.mock.calls[0][0]).toBe('/api/v1/bib/articles?limit=50&sort=created_at');
    });

    it('should not set method explicitly (defaults to GET)', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/articles');

      const init = mockFetch.mock.calls[0][1] as RequestInit;
      expect(init.method).toBeUndefined();
    });

    it('should allow overriding method', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/collections', { method: 'POST', body: { name: 'x' } });

      expect(mockFetch.mock.calls[0][1]).toEqual(
        expect.objectContaining({ method: 'POST' })
      );
    });

    it('should set Content-Type to application/json by default', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/collections', { method: 'POST', body: { name: 'x' } });

      const init = mockFetch.mock.calls[0][1] as { headers: Record<string, string> };
      expect(init.headers['Content-Type']).toBe('application/json');
    });

    it('should serialize an object body to JSON', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/chat', { method: 'POST', body: { scope: 'standalone', payload: { a: 1 } } });

      const init = mockFetch.mock.calls[0][1] as { body: string };
      expect(JSON.parse(init.body)).toEqual({ scope: 'standalone', payload: { a: 1 } });
    });

    it('should not override an existing Content-Type header', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/articles', {
        method: 'POST',
        body: { a: 1 },
        headers: { 'Content-Type': 'application/merge-patch+json' },
      });

      const init = mockFetch.mock.calls[0][1] as { headers: Record<string, string> };
      expect(init.headers['Content-Type']).toBe('application/merge-patch+json');
    });

    it('should not set Content-Type for FormData (browser sets the boundary)', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      const fd = new FormData();
      fd.append('file', new Blob(['x']), 'a.pdf');
      await request('/articles/upload', { method: 'POST', body: fd, headers: {} });

      const init = mockFetch.mock.calls[0][1] as { headers: Record<string, string> };
      expect(init.headers['Content-Type']).toBeUndefined();
    });
  });

  describe('Error Envelope (autonomics {"error": "..."})', () => {
    it('should surface the backend error string', async () => {
      global.fetch = makeFetch(() =>
        Promise.resolve(jsonResponse({ error: 'article not found' }, false, 404))
      ) as unknown as typeof fetch;

      await expect(request('/articles/xx')).rejects.toThrow('article not found');
    });

    it('should fall back to HTTP status when the body is not JSON', async () => {
      global.fetch = makeFetch(() =>
        Promise.resolve({
          ok: false,
          status: 500,
          statusText: 'Internal Server Error',
          headers: { get: () => null },
          json: () => Promise.reject(new Error('no body')),
        })
      ) as unknown as typeof fetch;

      await expect(request('/articles')).rejects.toThrow('Internal Server Error');
    });

    it('should still honour message/detail envelopes from intermediaries', async () => {
      global.fetch = makeFetch(() =>
        Promise.resolve(jsonResponse({ message: 'proxy says no' }, false, 502))
      ) as unknown as typeof fetch;

      await expect(request('/articles')).rejects.toThrow('proxy says no');
    });

    it('should throw a generic message when the envelope is empty', async () => {
      // statusText 也为空 → 所有的回落键都拿不到字符串，只能报状态码
      global.fetch = makeFetch(() =>
        Promise.resolve({ ...jsonResponse({}, false, 400), statusText: '' })
      ) as unknown as typeof fetch;

      await expect(request('/articles')).rejects.toThrow('请求失败: 400');
    });
  });

  describe('Success Handling', () => {
    it('should return the parsed JSON body verbatim (no envelope unwrapping)', async () => {
      const body = { total: 1, hits: [], articles: [{ id: 'doi:1' }], offset: 0, limit: 100 };
      global.fetch = makeFetch(() => Promise.resolve(jsonResponse(body))) as unknown as typeof fetch;

      await expect(request('/articles')).resolves.toEqual(body);
    });

    it('should return null for a 204 response', async () => {
      global.fetch = makeFetch(() =>
        Promise.resolve({
          ok: true,
          status: 204,
          statusText: 'No Content',
          headers: { get: () => null },
          json: () => Promise.resolve({}),
        })
      ) as unknown as typeof fetch;

      await expect(request('/articles/xx', { method: 'DELETE' })).resolves.toBeNull();
    });

    it('should return null when content-length is 0', async () => {
      global.fetch = makeFetch(() =>
        Promise.resolve({
          ok: true,
          status: 200,
          statusText: 'OK',
          headers: { get: () => '0' },
          json: () => Promise.resolve({}),
        })
      ) as unknown as typeof fetch;

      await expect(request('/articles')).resolves.toBeNull();
    });
  });

  describe('Timeout & Retry', () => {
    it('should expose the documented defaults', () => {
      expect(DEFAULT_TIMEOUT).toBe(30000);
      expect(DEFAULT_RETRIES).toBe(1);
    });

    it('should throw a friendly message on timeout (AbortError from our controller)', async () => {
      const mockFetch = vi.fn().mockImplementation(
        (_url: string, init: RequestInit) =>
          new Promise((_resolve, reject) => {
            init.signal?.addEventListener('abort', () => {
              const err = new Error('aborted');
              err.name = 'AbortError';
              reject(err);
            });
          })
      );
      global.fetch = mockFetch as unknown as typeof fetch;

      await expect(request('/articles', { timeout: 20 })).rejects.toThrow('请求超时（0.02秒）');
    }, 5000);

    it('should propagate an external abort without rewriting the message', async () => {
      const mockFetch = vi.fn().mockImplementation(
        (_url: string, init: RequestInit) =>
          new Promise((_resolve, reject) => {
            init.signal?.addEventListener('abort', () => {
              const err = new Error('aborted');
              err.name = 'AbortError';
              reject(err);
            });
          })
      );
      global.fetch = mockFetch as unknown as typeof fetch;

      const controller = new AbortController();
      const pending = request('/articles', { signal: controller.signal, timeout: 5000 });
      controller.abort();

      await expect(pending).rejects.toMatchObject({ name: 'AbortError' });
    });

    it('should retry once on a network TypeError then surface a retry message', async () => {
      const mockFetch = vi.fn().mockRejectedValue(new TypeError('Failed to fetch'));
      global.fetch = mockFetch as unknown as typeof fetch;

      await expect(request('/articles')).rejects.toThrow('网络请求失败（已重试 1 次）');
      expect(mockFetch).toHaveBeenCalledTimes(2);
    });

    it('should not retry on HTTP errors (only on network failures)', async () => {
      const mockFetch = makeFetch(() => Promise.resolve(jsonResponse({ error: 'nope' }, false, 404)));
      global.fetch = mockFetch as unknown as typeof fetch;

      await expect(request('/articles')).rejects.toThrow('nope');
      expect(mockFetch).toHaveBeenCalledTimes(1);
    });
  });

  describe('Caching & Invalidation', () => {
    it('should serve a repeated GET from cache (one fetch for two calls)', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/articles?limit=10');
      await request('/articles?limit=10');

      expect(mockFetch).toHaveBeenCalledTimes(1);
    });

    it('should deduplicate concurrent identical GETs', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await Promise.all([
        request('/collections'),
        request('/collections'),
      ]);

      expect(mockFetch).toHaveBeenCalledTimes(1);
    });

    it('should never cache non-GET requests', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/collections', { method: 'POST', body: { name: 'a' } });
      await request('/collections', { method: 'POST', body: { name: 'a' } });

      expect(mockFetch).toHaveBeenCalledTimes(2);
    });

    it('should distinguish different query strings in the cache key', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/articles?limit=10');
      await request('/articles?limit=20');

      expect(mockFetch).toHaveBeenCalledTimes(2);
    });

    it('invalidateCache(pattern) should drop matching entries only', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/collections');
      await request('/articles?limit=1');

      invalidateCache('/collections');

      await request('/collections');      // 已失效 → 重新 fetch (#3)
      await request('/articles?limit=1'); // 仍命中缓存

      // 2 次初始 + 1 次 /collections 失效后重取
      expect(mockFetch).toHaveBeenCalledTimes(3);
    });

    it('invalidateCache() with no argument should clear everything', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/collections');
      await request('/articles?limit=1');

      invalidateCache();

      await request('/collections');
      await request('/articles?limit=1');

      expect(mockFetch).toHaveBeenCalledTimes(4);
    });

    it('should apply the papers TTL bucket to /articles paths', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/articles?limit=1');
      // 人为把缓存条目变成"过期"：直接推进 timestamp 判断依赖的 Date.now()
      const realNow = Date.now;
      const now = realNow();
      vi.spyOn(Date, 'now').mockReturnValue(now + 3000); // > papers TTL (2000)

      await request('/articles?limit=1');
      vi.spyOn(Date, 'now').mockRestore();
      void realNow;

      expect(mockFetch).toHaveBeenCalledTimes(2);
    });
  });

  describe('Bearer Token', () => {
    it('should send no Authorization header when no token is stored', async () => {
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/articles');

      const init = mockFetch.mock.calls[0][1] as { headers: Record<string, string> };
      expect(init.headers.Authorization).toBeUndefined();
    });

    it('should attach Authorization when localStorage.autonomics_token exists', async () => {
      localStorage.setItem('autonomics_token', 'sekrit');
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/articles');

      const init = mockFetch.mock.calls[0][1] as { headers: Record<string, string> };
      expect(init.headers.Authorization).toBe('Bearer sekrit');
    });

    it('should ignore a whitespace-only token', async () => {
      localStorage.setItem('autonomics_token', '   ');
      const mockFetch = makeFetch();
      global.fetch = mockFetch as unknown as typeof fetch;

      await request('/articles');

      const init = mockFetch.mock.calls[0][1] as { headers: Record<string, string> };
      expect(init.headers.Authorization).toBeUndefined();
    });
  });

  describe('Environment Markers (compat exports)', () => {
    it('isTauri / isDesktop are always false in autonomics (web only)', () => {
      expect(isTauri).toBe(false);
      expect(isDesktop).toBe(false);
    });

    it('getTauriBaseUrl resolves to an empty string (same-origin)', async () => {
      await expect(getTauriBaseUrl()).resolves.toBe('');
    });
  });
});
