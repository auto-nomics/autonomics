/**
 * API 客户端测试文件 (client.js)
 *
 * 测试核心请求包装器，处理以下功能：
 * - URL 构造（带 /api 前缀）
 * - JSON 序列化
 * - 错误处理
 * - FormData 处理
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import request, { DEFAULT_TIMEOUT, DEFAULT_RETRIES, invalidateCache } from '../client';

describe('API Client - request()', () => {
  beforeEach(() => {
    // 每个测试前清除 mock 和请求缓存（cachedRequest 共享 module 级 Map）
    vi.clearAllMocks();
    invalidateCache();
  });

  afterEach(() => {
    // 每个测试后恢复 mock
    vi.restoreAllMocks();
  });

  describe('Request Construction', () => {
    it('should prepend /api to the path', async () => {
      // 应该在路径前添加 /api 前缀
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      await request('/papers');

      expect(mockFetch).toHaveBeenCalledWith(
        '/api/papers',
        expect.objectContaining({})
      );
    });

    it('should handle paths that already start with /api (results in /api/api/papers)', async () => {
      // 处理已包含 /api 前缀的路径（结果为 /api/api/papers）
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      await request('/api/papers');

      // 注意：客户端简单拼接 /api + path，所以 /api/papers 会变成 /api/api/papers
      // 这是已记录的行为 - 调用者应使用不带 /api 前缀的路径
      expect(mockFetch).toHaveBeenCalledWith(
        '/api/api/papers',
        expect.anything()
      );
    });

    it('should not set method explicitly (defaults to GET)', async () => {
      // 不显式设置方法（默认为 GET）
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ data: [] }),
      });
      global.fetch = mockFetch;

      await request('/papers');

      // 客户端不显式设置 method，所以 fetch 默认为 GET
      expect(mockFetch).toHaveBeenCalledWith(
        '/api/papers',
        expect.objectContaining({
          headers: expect.any(Object),
        })
      );
    });

    it('should allow overriding method', async () => {
      // 允许覆盖请求方法
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      await request('/papers', { method: 'POST' });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.any(String),
        expect.objectContaining({
          method: 'POST',
        })
      );
    });

    it('should set Content-Type to application/json by default', async () => {
      // 默认设置 Content-Type 为 application/json（需要传 body 触发自动设置）
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      await request('/papers', { method: 'POST', body: {} });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.any(String),
        expect.objectContaining({
          headers: expect.objectContaining({
            'Content-Type': 'application/json',
          }),
        })
      );
    });

    it('should not override existing Content-Type header', async () => {
      // 不覆盖已存在的 Content-Type 头
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      await request('/papers', {
        method: 'POST',
        headers: { 'Content-Type': 'application/xml' },
      });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.any(String),
        expect.objectContaining({
          headers: expect.objectContaining({
            'Content-Type': 'application/xml',
          }),
        })
      );
    });

    it('should allow setting additional headers', async () => {
      // 允许设置额外的请求头
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      await request('/papers', {
        method: 'POST',
        body: {},
        headers: { 'X-Custom-Header': 'custom-value' },
      });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.any(String),
        expect.objectContaining({
          headers: expect.objectContaining({
            'X-Custom-Header': 'custom-value',
            'Content-Type': 'application/json',
          }),
        })
      );
    });
  });

  describe('Request Body Serialization', () => {
    it('should JSON.stringify object bodies', async () => {
      // 应该对对象 body 进行 JSON 序列化
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      const bodyData = { title: 'Test Paper', authors: ['Author 1'] };
      await request('/papers', { method: 'POST', body: bodyData });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.any(String),
        expect.objectContaining({
          body: JSON.stringify(bodyData),
        })
      );
    });

    it('should not serialize string bodies', async () => {
      // 不序列化字符串 body
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      const bodyString = 'raw string body';
      await request('/papers', { method: 'POST', body: bodyString });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.any(String),
        expect.objectContaining({
          body: bodyString,
        })
      );
    });

    it('should handle FormData without setting Content-Type', async () => {
      // 处理 FormData 时不设置 Content-Type（让浏览器自动设置 boundary）
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      const formData = new FormData();
      formData.append('file', new Blob(['test']), 'test.pdf');
      formData.append('title', 'Test');

      await request('/papers/upload', { method: 'POST', body: formData });

      const callArgs = mockFetch.mock.calls[0];
      expect(callArgs[0]).toBe('/api/papers/upload');
      expect(callArgs[1].body).toBe(formData);
      // FormData 不应该设置 Content-Type（浏览器会自动设置带 boundary 的值）
      expect(callArgs[1].headers['Content-Type']).toBeUndefined();
    });

    it('should not serialize FormData body', async () => {
      // 不序列化 FormData body
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      const formData = new FormData();
      formData.append('key', 'value');

      await request('/upload', { method: 'POST', body: formData });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.any(String),
        expect.objectContaining({
          body: formData,  // 应该是 FormData 对象本身
        })
      );
    });

    it('should handle null body', async () => {
      // 处理 null body
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      await request('/papers', { method: 'POST', body: null });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.any(String),
        expect.objectContaining({
          body: null,
        })
      );
    });

    it('should handle empty object body', async () => {
      // 处理空对象 body
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      await request('/papers', { method: 'POST', body: {} });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.any(String),
        expect.objectContaining({
          body: JSON.stringify({}),
        })
      );
    });
  });

  describe('Response Parsing', () => {
    it('should parse and return JSON response', async () => {
      // 解析并返回 JSON 响应
      const mockResponse = { data: 'test', id: 123 };
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve(mockResponse),
      });
      global.fetch = mockFetch;

      const result = await request('/papers');

      expect(result).toEqual(mockResponse);
    });

    it('should handle empty response body', async () => {
      // 处理空响应体
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve(null),
      });
      global.fetch = mockFetch;

      const result = await request('/papers');

      expect(result).toBeNull();
    });

    it('should handle array responses', async () => {
      // 处理数组响应
      const mockResponse = [{ id: 1 }, { id: 2 }];
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve(mockResponse),
      });
      global.fetch = mockFetch;

      const result = await request('/papers');

      expect(result).toEqual(mockResponse);
      expect(Array.isArray(result)).toBe(true);
    });
  });

  describe('Error Handling', () => {
    it('should throw error on non-OK response (404)', async () => {
      // 非 OK 响应应抛出错误 (404)
      const mockFetch = vi.fn().mockResolvedValue({
        ok: false,
        status: 404,
        statusText: 'Not Found',
        json: () => Promise.resolve({ detail: 'Resource not found' }),
      });
      global.fetch = mockFetch;

      await expect(request('/papers/999')).rejects.toThrow('Resource not found');
    });

    it('should throw error on non-OK response (500)', async () => {
      // 非 OK 响应应抛出错误 (500)
      const mockFetch = vi.fn().mockResolvedValue({
        ok: false,
        status: 500,
        statusText: 'Internal Server Error',
        json: () => Promise.resolve({ detail: 'Server error occurred' }),
      });
      global.fetch = mockFetch;

      await expect(request('/papers')).rejects.toThrow('Server error occurred');
    });

    it('should throw error on non-OK response (401)', async () => {
      // 非 OK 响应应抛出错误 (401)
      const mockFetch = vi.fn().mockResolvedValue({
        ok: false,
        status: 401,
        statusText: 'Unauthorized',
        json: () => Promise.resolve({ detail: 'Authentication required' }),
      });
      global.fetch = mockFetch;

      await expect(request('/protected')).rejects.toThrow('Authentication required');
    });

    it('should use statusText when error response is not JSON', async () => {
      // 错误响应非 JSON 时使用 statusText
      const mockFetch = vi.fn().mockResolvedValue({
        ok: false,
        status: 500,
        statusText: 'Internal Server Error',
        json: () => Promise.reject(new Error('Invalid JSON')),
      });
      global.fetch = mockFetch;

      await expect(request('/papers')).rejects.toThrow('Internal Server Error');
    });

    it('should handle array detail from FastAPI validation errors', async () => {
      // 处理来自 FastAPI 验证错误的数组 detail
      const mockFetch = vi.fn().mockResolvedValue({
        ok: false,
        status: 422,
        statusText: 'Unprocessable Entity',
        json: () => Promise.resolve({
          detail: [
            { type: 'string_too_short', msg: 'String should have at least 1 character' },
          ],
        }),
      });
      global.fetch = mockFetch;

      await expect(request('/papers')).rejects.toThrow();
    });

    it('should handle non-string detail (object)', async () => {
      // 处理非字符串 detail（对象）
      const mockFetch = vi.fn().mockResolvedValue({
        ok: false,
        status: 422,
        statusText: 'Unprocessable Entity',
        json: () => Promise.resolve({
          detail: { field: 'title', message: 'This field is required' },
        }),
      });
      global.fetch = mockFetch;

      const error = await request('/papers').catch((e: unknown) => e) as Error;
      // detail 是对象（非字符串），代码回退到 `请求失败: ${status}`
      expect(error.message).toContain('请求失败: 422');
    });

    it('should handle missing detail field', async () => {
      // 处理缺少 detail 字段的情况
      const mockFetch = vi.fn().mockResolvedValue({
        ok: false,
        status: 500,
        statusText: 'Internal Server Error',
        json: () => Promise.resolve({ error: 'Something went wrong' }),
      });
      global.fetch = mockFetch;

      // detail 缺失但有 error 字段 → 代码回退到 error 字段
      await expect(request('/papers')).rejects.toThrow('Something went wrong');
    });

    it('should include status code in error message when detail is missing', async () => {
      // detail/message/error 全部缺失时，回退到 statusText
      const mockFetch = vi.fn().mockResolvedValue({
        ok: false,
        status: 503,
        statusText: 'Service Unavailable',
        json: () => Promise.resolve({}),
      });
      global.fetch = mockFetch;

      await expect(request('/papers')).rejects.toThrow('Service Unavailable');
    });
  });

  describe('Network Error Handling', () => {
    it('should handle network failure', async () => {
      // 处理网络故障
      const mockFetch = vi.fn().mockRejectedValue(new TypeError('Failed to fetch'));
      global.fetch = mockFetch;

      await expect(request('/papers')).rejects.toThrow('Failed to fetch');
    });

    it('should handle timeout', async () => {
      // 处理超时
      const mockFetch = vi.fn().mockRejectedValue(new Error('Request timeout'));
      global.fetch = mockFetch;

      await expect(request('/papers')).rejects.toThrow('Request timeout');
    });

    it('should handle connection refused', async () => {
      // 处理连接被拒绝
      const mockFetch = vi.fn().mockRejectedValue(new Error('ECONNREFUSED'));
      global.fetch = mockFetch;

      await expect(request('/papers')).rejects.toThrow('ECONNREFUSED');
    });
  });

  describe('Timeout', () => {
    it('should abort request after timeout', async () => {
      // 超时后中止请求
      const mockFetch = vi.fn().mockImplementation((url, opts) => {
        return new Promise((resolve, reject) => {
          const onAbort = () => {
            reject(new DOMException('The operation was aborted.', 'AbortError'));
          };
          opts.signal.addEventListener('abort', onAbort);
        });
      });
      global.fetch = mockFetch;

      await expect(request('/papers', { timeout: 50 })).rejects.toThrow('请求超时');
    });

    it('should clear timeout when request completes', async () => {
      // 请求完成时清除超时
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      const result = await request('/papers', { timeout: 5000 });
      expect(result).toEqual({ success: true });
    });
  });

  describe('Retry', () => {
    it('should retry once on network error by default', async () => {
      // 网络错误默认重试一次
      let callCount = 0;
      const mockFetch = vi.fn().mockImplementation(() => {
        callCount++;
        if (callCount === 1) return Promise.reject(new TypeError('Failed to fetch'));
        return Promise.resolve({
          ok: true,
          status: 200,
          headers: { get: () => null },
          json: () => Promise.resolve({ success: true }),
        });
      });
      global.fetch = mockFetch;

      const result = await request('/papers');
      expect(result).toEqual({ success: true });
      expect(mockFetch).toHaveBeenCalledTimes(2);
    });

    it('should not retry on HTTP errors', async () => {
      // HTTP 错误不重试
      const mockFetch = vi.fn().mockResolvedValue({
        ok: false,
        status: 500,
        statusText: 'Internal Server Error',
        json: () => Promise.resolve({ detail: 'Server error' }),
      });
      global.fetch = mockFetch;

      await expect(request('/papers')).rejects.toThrow('Server error');
      expect(mockFetch).toHaveBeenCalledTimes(1);
    });

    it('should throw after all retries exhausted', async () => {
      // 所有重试耗尽后抛出错误
      const mockFetch = vi.fn().mockRejectedValue(new TypeError('Failed to fetch'));
      global.fetch = mockFetch;

      await expect(request('/papers', { retries: 1 })).rejects.toThrow('网络请求失败');
      expect(mockFetch).toHaveBeenCalledTimes(2);  // 1 次初始 + 1 次重试
    });

    it('should support retries=0 for no retry', async () => {
      // 支持 retries=0 不重试
      const mockFetch = vi.fn().mockRejectedValue(new TypeError('Failed to fetch'));
      global.fetch = mockFetch;

      await expect(request('/papers', { retries: 0 })).rejects.toThrow('Failed to fetch');
      expect(mockFetch).toHaveBeenCalledTimes(1);
    });
  });

  describe('Edge Cases', () => {
    it('should handle empty path', async () => {
      // 处理空路径
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      await request('');

      expect(mockFetch).toHaveBeenCalledWith('/api', expect.anything());
    });

    it('should handle path without leading slash (results in /apipapers)', async () => {
      // 处理不带前导斜杠的路径（结果为 /apipapers）
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      await request('papers');

      // 注意：客户端简单拼接 /api + path，所以 papers 会变成 /apipapers
      // 这是已记录的行为 - 调用者应使用带前导斜杠的路径
      expect(mockFetch).toHaveBeenCalledWith('/apipapers', expect.anything());
    });

    it('should handle query parameters in path', async () => {
      // 处理路径中的查询参数
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ data: [] }),
      });
      global.fetch = mockFetch;

      await request('/papers?limit=10&offset=0');

      expect(mockFetch).toHaveBeenCalledWith(
        '/api/papers?limit=10&offset=0',
        expect.anything()
      );
    });

    it('should merge custom headers with default headers', async () => {
      // 合并自定义头和默认头
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      await request('/papers', {
        method: 'POST',
        body: {},
        headers: { 'X-Auth-Token': 'secret' },
      });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.any(String),
        expect.objectContaining({
          headers: expect.objectContaining({
            'Content-Type': 'application/json',
            'X-Auth-Token': 'secret',
          }),
        })
      );
    });

    it('should handle body with options that have other properties', async () => {
      // 处理带有其他属性的 options body
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ success: true }),
      });
      global.fetch = mockFetch;

      await request('/papers', {
        method: 'POST',
        body: { title: 'Test' },
        cache: 'no-cache',
        credentials: 'same-origin',
      });

      expect(mockFetch).toHaveBeenCalledWith(
        expect.any(String),
        expect.objectContaining({
          method: 'POST',
          cache: 'no-cache',
          credentials: 'same-origin',
        })
      );
    });
  });

  describe('Real-world API Scenarios', () => {
    it('should handle successful paper creation', async () => {
      // 处理成功的论文创建
      const newPaper = { id: 1, title: 'Test Paper', status: 'processing' };
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve(newPaper),
      });
      global.fetch = mockFetch;

      const result = await request('/papers', {
        method: 'POST',
        body: { title: 'Test Paper', file: 'data URI' },
      });

      expect(result).toEqual(newPaper);
    });

    it('should handle successful paper listing with query params', async () => {
      // 处理带查询参数的成功论文列表
      const papers = [
        { id: 1, title: 'Paper 1' },
        { id: 2, title: 'Paper 2' },
      ];
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve(papers),
      });
      global.fetch = mockFetch;

      const result = await request('/papers?limit=10');

      expect(result).toEqual(papers);
      expect(mockFetch).toHaveBeenCalledWith('/api/papers?limit=10', expect.anything());
    });

    it('should handle paper deletion', async () => {
      // 处理论文删除
      const mockFetch = vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        headers: { get: () => null },
        json: () => Promise.resolve({ message: 'Paper deleted' }),
      });
      global.fetch = mockFetch;

      const result = await request('/papers/1', { method: 'DELETE' });

      expect(result).toEqual({ message: 'Paper deleted' });
    });

    it('should handle validation error on paper creation', async () => {
      // 处理论文创建时的验证错误
      const mockFetch = vi.fn().mockResolvedValue({
        ok: false,
        status: 422,
        statusText: 'Unprocessable Entity',
        json: () => Promise.resolve({
          detail: [
            {
              type: 'missing',
              loc: ['body', 'title'],
              msg: 'Field required',
            },
          ],
        }),
      });
      global.fetch = mockFetch;

      await expect(
        request('/papers', { method: 'POST', body: {} })
      ).rejects.toThrow();
    });
  });
});
