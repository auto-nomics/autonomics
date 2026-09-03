/**
 * Highlights API 测试
 *
 * 测试高亮标注相关的后端接口调用，包括：
 * - 获取高亮列表（支持按页码过滤）
 * - 创建新的高亮标注
 * - 更新高亮的颜色
 * - 删除指定的高亮标注
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { FetchHighlightsResponse, Highlight } from '@/types';
import { invalidateCache } from '../client';
import {
  fetchHighlights,
  createHighlight,
  updateHighlight,
  deleteHighlight,
} from '../highlightsApi';

// ============================================================================
// 测试环境设置
// ============================================================================

// Mock 全局 fetch 函数
const mockFetch = vi.fn();
global.fetch = mockFetch;

// Mock API_BASE 常量
const API_BASE = '/api';

// beforeEach 钩子：每个测试前重置 mock 和缓存
beforeEach(() => {
  mockFetch.mockClear();
  invalidateCache();
});

// ============================================================================
// fetchHighlights 测试
// ============================================================================

describe('fetchHighlights — 获取高亮列表', () => {
  // 基础获取功能（不带页码过滤）
  it('发送 GET 请求获取论文的全部高亮', async () => {
    const mockResponse = {
      highlights: [
        {
          id: 1,
          paper_id: 123,
          page: 2,
          text: '示例文本',
          color: '#ffe066',
          rects: [{ x: 0.1, y: 0.2, w: 0.3, h: 0.02 }],
          created_at: 1234567890,
        },
      ],
      total: 1,
    };
    mockFetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: { get: () => null },
      json: async () => mockResponse,
    });

    const result = await fetchHighlights('123') as unknown as FetchHighlightsResponse;

    expect(mockFetch).toHaveBeenCalledTimes(1);
    expect(mockFetch.mock.calls[0][0]).toBe(`${API_BASE}/papers/123/highlights`);
    expect(result).toEqual(mockResponse);
  });

  // 带页码过滤
  it('支持按页码过滤高亮（添加 page 查询参数）', async () => {
    const mockResponse = {
      highlights: [
        {
          id: 1,
          paper_id: 123,
          page: 2,
          text: '第2页的文本',
          color: '#ffe066',
          rects: [{ x: 0.1, y: 0.2, w: 0.3, h: 0.02 }],
          created_at: 1234567890,
        },
      ],
      total: 1,
    };
    mockFetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: { get: () => null },
      json: async () => mockResponse,
    });

    const result = await fetchHighlights('123', 2) as unknown as FetchHighlightsResponse;

    expect(mockFetch).toHaveBeenCalledTimes(1);
    expect(mockFetch.mock.calls[0][0]).toBe(`${API_BASE}/papers/123/highlights?page=2`);
    expect(result).toEqual(mockResponse);
  });

  // page 为 null 时不添加查询参数
  it('page 为 null 时不添加查询参数', async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: { get: () => null },
      json: async () => ({ highlights: [], total: 0 }),
    });

    await fetchHighlights('123', null);

    const url = mockFetch.mock.calls[0][0];
    expect(url).toBe(`${API_BASE}/papers/123/highlights`);
    expect(url).not.toContain('?');
  });

  // page 不传时默认为 null（获取全部高亮）
  it('不传 page 参数时获取全部高亮', async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: { get: () => null },
      json: async () => ({ highlights: [], total: 0 }),
    });

    await fetchHighlights('123');

    const url = mockFetch.mock.calls[0][0];
    expect(url).toBe(`${API_BASE}/papers/123/highlights`);
    expect(url).not.toContain('?');
  });

  // 返回空数组时处理正确
  it('正确返回空的高亮数组', async () => {
    const mockResponse = { highlights: [], total: 0 };
    mockFetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: { get: () => null },
      json: async () => mockResponse,
    });

    const result = await fetchHighlights('123') as unknown as FetchHighlightsResponse;

    expect(result).toEqual(mockResponse);
    expect(result.highlights).toEqual([]);
    expect(result.total).toBe(0);
  });

  // 多个矩形的高亮
  it('正确解析包含多个矩形的高亮数据', async () => {
    const mockResponse = {
      highlights: [
        {
          id: 1,
          paper_id: 123,
          page: 1,
          text: '跨行选中的文本',
          color: '#ffec3d',
          rects: [
            { x: 0.1, y: 0.2, w: 0.3, h: 0.02 },
            { x: 0.1, y: 0.23, w: 0.25, h: 0.02 },
          ],
          created_at: 1234567890,
        },
      ],
      total: 1,
    };
    mockFetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: { get: () => null },
      json: async () => mockResponse,
    });

    const result = await fetchHighlights('123') as unknown as FetchHighlightsResponse;

    expect(result.highlights[0].rects).toHaveLength(2);
    expect(result.highlights[0].rects[0]).toEqual({ x: 0.1, y: 0.2, w: 0.3, h: 0.02 });
    expect(result.highlights[0].rects[1]).toEqual({ x: 0.1, y: 0.23, w: 0.25, h: 0.02 });
  });
});

// ============================================================================
// createHighlight 测试
// ============================================================================

describe('createHighlight — 创建新的高亮标注', () => {
  // 基础创建功能
  it('发送 POST 请求创建高亮', async () => {
    const highlightData = {
      page: 2,
      text: '选中的文本',
      rects: [{ x: 0.1, y: 0.2, w: 0.3, h: 0.02 }],
      color: '#ffe066',
    };
    const mockResponse = {
      id: 1,
      paper_id: 123,
      ...highlightData,
      created_at: 1234567890,
    };
    mockFetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: { get: () => null },
      json: async () => mockResponse,
    });

    const result = await createHighlight('123', highlightData) as unknown as Highlight;

    expect(mockFetch).toHaveBeenCalledTimes(1);
    const callArgs = mockFetch.mock.calls[0];
    expect(callArgs[0]).toBe(`${API_BASE}/papers/123/highlights`);
    expect(callArgs[1].method).toBe('POST');
    // request 函数会将 body 序列化为 JSON 字符串
    expect(callArgs[1].body).toBe(JSON.stringify(highlightData));
    expect(result).toEqual(mockResponse);
  });

  // 多个矩形的高亮
  it('支持创建包含多个矩形的高亮（跨行选区）', async () => {
    const highlightData = {
      page: 1,
      text: '跨行选中的文本',
      rects: [
        { x: 0.1, y: 0.2, w: 0.3, h: 0.02 },
        { x: 0.1, y: 0.23, w: 0.25, h: 0.02 },
        { x: 0.1, y: 0.26, w: 0.2, h: 0.02 },
      ],
      color: '#ffe066',
    };
    mockFetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: { get: () => null },
      json: async () => ({ id: 1, paper_id: 123, ...highlightData, created_at: 1234567890 }),
    });

    const result = await createHighlight('123', highlightData) as unknown as Highlight;

    expect(result.rects).toHaveLength(3);
    // request 函数会将 body 序列化为 JSON 字符串，需要解析后验证
    const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);
    expect(requestBody.rects).toHaveLength(3);
  });

  // 不同颜色的高亮
  it('支持不同的高亮颜色', async () => {
    const highlightData = {
      page: 1,
      text: '已理解的内容',
      rects: [{ x: 0.1, y: 0.2, w: 0.3, h: 0.02 }],
      color: '#b7eb8f', // 绿色表示"已理解"
    };
    mockFetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: { get: () => null },
      json: async () => ({ id: 1, paper_id: 123, ...highlightData, created_at: 1234567890 }),
    });

    const result = await createHighlight('123', highlightData) as unknown as Highlight;

    expect(result.color).toBe('#b7eb8f');
  });
});

// ============================================================================
// updateHighlight 测试
// ============================================================================

describe('updateHighlight — 更新高亮', () => {
  // 更新颜色
  it('发送 PUT 请求更新高亮颜色', async () => {
    const updateData = { color: '#b7eb8f' };
    const mockResponse = {
      id: 1,
      paper_id: 123,
      page: 2,
      text: '示例文本',
      rects: [{ x: 0.1, y: 0.2, w: 0.3, h: 0.02 }],
      color: '#b7eb8f',
      created_at: 1234567890,
    };
    mockFetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: { get: () => null },
      json: async () => mockResponse,
    });

    const result = await updateHighlight('123', '1', updateData);

    expect(mockFetch).toHaveBeenCalledTimes(1);
    const callArgs = mockFetch.mock.calls[0];
    expect(callArgs[0]).toBe(`${API_BASE}/papers/123/highlights/1`);
    expect(callArgs[1].method).toBe('PUT');
    // request 函数会将 body 序列化为 JSON 字符串
    expect(callArgs[1].body).toBe(JSON.stringify(updateData));
    expect(result.color).toBe('#b7eb8f');
  });
});

// ============================================================================
// deleteHighlight 测试
// ============================================================================

describe('deleteHighlight — 删除高亮', () => {
  // 基础删除功能
  it('发送 DELETE 请求删除指定高亮', async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: { get: () => null },
      status: 204,
      json: async () => ({}), // 204 响应体可能为空
    });

    await deleteHighlight('123', '1');

    expect(mockFetch).toHaveBeenCalledTimes(1);
    const callArgs = mockFetch.mock.calls[0];
    expect(callArgs[0]).toBe(`${API_BASE}/papers/123/highlights/1`);
    expect(callArgs[1].method).toBe('DELETE');
  });

  // 删除后返回 undefined（204 无内容）
  it('删除成功后返回 undefined（204 状态码）', async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: { get: () => null },
      status: 204,
      json: async () => ({}),
    });

    const result = await deleteHighlight('123', '1');

    // 204 响应通常没有响应体
    expect(result).toBeDefined();
  });

  // 不同的高亮 ID
  it('正确构建不同高亮 ID 的 URL', async () => {
    mockFetch.mockResolvedValueOnce({
      ok: true,
      status: 200,
      headers: { get: () => null },
      status: 204,
      json: async () => ({}),
    });

    await deleteHighlight('123', '999');

    expect(mockFetch.mock.calls[0][0]).toBe(`${API_BASE}/papers/123/highlights/999`);
  });
});

// ============================================================================
// 错误处理测试
// ============================================================================

describe('错误处理', () => {
  it('fetchHighlights 请求失败时抛出错误', async () => {
    mockFetch.mockResolvedValueOnce({
      ok: false,
      status: 500,
      json: async () => ({ detail: '服务器内部错误' }),
    });

    await expect(fetchHighlights('123')).rejects.toThrow('服务器内部错误');
  });

  it('createHighlight 请求失败时抛出错误', async () => {
    const highlightData = {
      page: 1,
      text: '测试',
      rects: [{ x: 0.1, y: 0.2, w: 0.3, h: 0.02 }],
      color: '#ffe066',
    };
    mockFetch.mockResolvedValueOnce({
      ok: false,
      status: 400,
      json: async () => ({ detail: '无效的矩形坐标' }),
    });

    await expect(createHighlight('123', highlightData)).rejects.toThrow('无效的矩形坐标');
  });

  it('updateHighlight 请求失败时抛出错误', async () => {
    mockFetch.mockResolvedValueOnce({
      ok: false,
      status: 404,
      json: async () => ({ detail: '高亮不存在' }),
    });

    await expect(updateHighlight('123', '999', { color: '#b7eb8f' })).rejects.toThrow('高亮不存在');
  });

  it('deleteHighlight 请求失败时抛出错误', async () => {
    mockFetch.mockResolvedValueOnce({
      ok: false,
      status: 404,
      json: async () => ({ detail: '高亮不存在' }),
    });

    await expect(deleteHighlight('123', '999')).rejects.toThrow('高亮不存在');
  });
});
