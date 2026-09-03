/**
 * Papers API 测试
 *
 * 测试论文相关的后端接口调用，包括：
 * - 论文上传与 CRUD 操作
 * - PDF 文件获取
 * - Markdown 解析内容获取
 * - 论文重解析（支持多种解析引擎）
 * - 翻译功能
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  uploadPaper,
  getPapers,
  getPaper,
  getPdfUrl,
  getMarkdown,
  reparsePaper,
  deletePaper,
  getStreamUrl,
  fetchMetadata,
  checkMetadataPending,
  getPaperTypes,
  translatePaper,
} from '../papersApi';

// 使用工厂函数创建 mock，避免变量提升问题
// 这个工厂函数会在 vi.mock 被提升时执行
const requestMocks = vi.hoisted(() => ({
  mockRequest: vi.fn(),
}));

vi.mock('../client', () => ({
  default: requestMocks.mockRequest,
  invalidateCache: vi.fn(),
}));

// ============================================================================
// 测试环境设置
// ============================================================================

// beforeEach 钩子：每个测试前重置 mock
beforeEach(() => {
  requestMocks.mockRequest.mockClear();
  requestMocks.mockRequest.mockResolvedValue({});
});

// 重置全局 fetch mock
afterEach(() => {
  vi.restoreAllMocks();
});

// ============================================================================
// uploadPaper 测试
// ============================================================================

describe('uploadPaper — 上传 PDF 文件', () => {
  // 基础上传功能
  it('正确调用 request 发送 FormData', async () => {
    const mockFile = new File(['pdf content'], 'test.pdf', { type: 'application/pdf' });
    const expectedResponse = { id: 1, parse_status: 'pending', title: 'test.pdf' };
    requestMocks.mockRequest.mockResolvedValueOnce(expectedResponse);

    const result = await uploadPaper(mockFile);

    expect(requestMocks.mockRequest).toHaveBeenCalledTimes(1);
    expect(requestMocks.mockRequest.mock.calls[0][0]).toBe('/papers/upload');
    expect(requestMocks.mockRequest.mock.calls[0][1].method).toBe('POST');
    expect(requestMocks.mockRequest.mock.calls[0][1].body).toBeInstanceOf(FormData);
    expect(requestMocks.mockRequest.mock.calls[0][1].headers).toEqual({});
    expect(result).toEqual(expectedResponse);
  });

  // 带分类 ID 上传
  it('支持指定 categoryId 参数', async () => {
    const mockFile = new File(['pdf content'], 'test.pdf', { type: 'application/pdf' });
    requestMocks.mockRequest.mockResolvedValueOnce({ id: 1 });

    await uploadPaper(mockFile, '5');

    const formData = requestMocks.mockRequest.mock.calls[0][1].body;
    expect(formData.get('category_id')).toBe('5');
  });

  // categoryId 为 null 时不发送该字段
  it('categoryId 为 null 时不添加到 FormData', async () => {
    const mockFile = new File(['pdf content'], 'test.pdf', { type: 'application/pdf' });
    requestMocks.mockRequest.mockResolvedValueOnce({ id: 1 });

    await uploadPaper(mockFile, null);

    const formData = requestMocks.mockRequest.mock.calls[0][1].body;
    expect(formData.get('category_id')).toBeNull();
  });
});

// ============================================================================
// getPapers 测试
// ============================================================================

describe('getPapers — 获取论文列表', () => {
  // 基础列表获取
  it('发送 GET 请求获取论文列表', async () => {
    const expectedResponse = { papers: [], total: 0 };
    requestMocks.mockRequest.mockResolvedValueOnce(expectedResponse);

    const result = await getPapers();

    expect(requestMocks.mockRequest).toHaveBeenCalledTimes(1);
    expect(requestMocks.mockRequest.mock.calls[0][0]).toBe('/papers');
    expect(result).toEqual(expectedResponse);
  });

  // 带查询参数
  it('正确构建查询字符串', async () => {
    requestMocks.mockRequest.mockResolvedValueOnce({ papers: [], total: 0 });

    await getPapers({ sort: 'title', order: 'asc', paper_type: 'journal' });

    const url = requestMocks.mockRequest.mock.calls[0][0];
    expect(url).toContain('sort=title');
    expect(url).toContain('order=asc');
    expect(url).toContain('paper_type=journal');
  });

  // 空参数对象
  it('空参数对象不添加查询字符串', async () => {
    requestMocks.mockRequest.mockResolvedValueOnce({ papers: [], total: 0 });

    await getPapers({});

    const url = requestMocks.mockRequest.mock.calls[0][0];
    expect(url).toBe('/papers');
    expect(url).not.toContain('?');
  });
});

// ============================================================================
// getPaper 测试
// ============================================================================

describe('getPaper — 获取单篇论文详情', () => {
  it('发送 GET 请求获取指定论文详情', async () => {
    const expectedData = { id: 1, title: 'Test Paper', authors: ['Author 1'] };
    requestMocks.mockRequest.mockResolvedValueOnce(expectedData);

    const result = await getPaper('1');

    expect(requestMocks.mockRequest).toHaveBeenCalledTimes(1);
    expect(requestMocks.mockRequest.mock.calls[0][0]).toBe('/papers/1');
    expect(result).toEqual(expectedData);
  });
});

// ============================================================================
// getPdfUrl 测试
// ============================================================================

describe('getPdfUrl — 获取 PDF 文件 URL', () => {
  it('返回 PDF 文件的正确 URL（同步函数）', () => {
    const url = getPdfUrl('123');

    expect(url).toBe('/api/papers/123/pdf');
    expect(typeof url).toBe('string');
  });

  it('不同论文 ID 返回不同 URL', () => {
    const url1 = getPdfUrl('1');
    const url2 = getPdfUrl('999');

    expect(url1).not.toBe(url2);
    expect(url1).toContain('/papers/1/pdf');
    expect(url2).toContain('/papers/999/pdf');
  });
});

// ============================================================================
// getMarkdown 测试
// ============================================================================

describe('getMarkdown — 获取 Markdown 内容', () => {
  it('发送 GET 请求获取论文解析后的 Markdown', async () => {
    const expectedData = { markdown: '# Title\n\nContent', parse_status: 'completed' };
    requestMocks.mockRequest.mockResolvedValueOnce(expectedData);

    const result = await getMarkdown('5');

    expect(requestMocks.mockRequest).toHaveBeenCalledTimes(1);
    expect(requestMocks.mockRequest.mock.calls[0][0]).toBe('/papers/5/markdown');
    expect(result).toEqual(expectedData);
  });
});

// ============================================================================
// reparsePaper 测试
// ============================================================================

describe('reparsePaper — 重解析论文', () => {
  it('发送 POST 请求触发重解析', async () => {
    requestMocks.mockRequest.mockResolvedValueOnce({ message: 'Reparse started' });

    await reparsePaper('1', 'mineru');

    expect(requestMocks.mockRequest).toHaveBeenCalledTimes(1);
    expect(requestMocks.mockRequest.mock.calls[0][0]).toBe('/papers/1/reparse');
    expect(requestMocks.mockRequest.mock.calls[0][1].method).toBe('POST');
    expect(requestMocks.mockRequest.mock.calls[0][1].body).toEqual({ engine: 'mineru' });
  });

  // 默认使用 mineru 引擎
  it('默认使用 mineru 引擎', async () => {
    requestMocks.mockRequest.mockResolvedValueOnce({ message: 'Reparse started' });

    await reparsePaper('1');

    expect(requestMocks.mockRequest.mock.calls[0][1].body).toEqual({ engine: 'mineru' });
  });
});

// ============================================================================
// deletePaper 测试
// ============================================================================

describe('deletePaper — 删除论文', () => {
  it('发送 DELETE 请求删除指定论文', async () => {
    requestMocks.mockRequest.mockResolvedValueOnce({ message: 'Paper deleted' });

    await deletePaper('42');

    expect(requestMocks.mockRequest).toHaveBeenCalledTimes(1);
    expect(requestMocks.mockRequest.mock.calls[0][0]).toBe('/papers/42');
    expect(requestMocks.mockRequest.mock.calls[0][1].method).toBe('DELETE');
  });
});

// ============================================================================
// getStreamUrl 测试
// ============================================================================

describe('getStreamUrl — 获取 SSE 端点 URL', () => {
  it('返回 SSE 实时推送端点的正确 URL（同步函数）', () => {
    const url = getStreamUrl('10');

    expect(url).toBe('/api/papers/10/stream');
    expect(typeof url).toBe('string');
  });
});

// ============================================================================
// fetchMetadata 测试
// ============================================================================

describe('fetchMetadata — 获取期刊元数据', () => {
  it('发送 POST 请求触发元数据获取', async () => {
    requestMocks.mockRequest.mockResolvedValueOnce({ message: 'Metadata fetch started' });

    await fetchMetadata('1');

    expect(requestMocks.mockRequest).toHaveBeenCalledTimes(1);
    expect(requestMocks.mockRequest.mock.calls[0][0]).toBe('/metadata/1/fetch');
    expect(requestMocks.mockRequest.mock.calls[0][1].method).toBe('POST');
  });
});

// ============================================================================
// checkMetadataPending 测试
// ============================================================================

describe('checkMetadataPending — 检查元数据获取状态', () => {
  it('调用 pending 端点获取所有正在处理的 ID', async () => {
    requestMocks.mockRequest.mockResolvedValueOnce({ pending_ids: [1, 3] });

    await checkMetadataPending();

    const url = requestMocks.mockRequest.mock.calls[0][0];
    expect(url).toBe('/metadata/pending');
  });

  it('无参数调用', async () => {
    requestMocks.mockRequest.mockResolvedValueOnce({ pending_ids: [] });

    await checkMetadataPending();

    expect(requestMocks.mockRequest).toHaveBeenCalledTimes(1);
  });
});

// ============================================================================
// getPaperTypes 测试
// ============================================================================

describe('getPaperTypes — 获取支持的文献类型列表', () => {
  it('调用 request 获取类型列表', async () => {
    // 准备 mock 返回的文献类型数据
    const expectedTypes = {
      journal: { name: '期刊论文', name_en: 'Journal Article' },
    };
    // 设置 mock request 返回类型数据
    requestMocks.mockRequest.mockResolvedValueOnce(expectedTypes);

    const result = await getPaperTypes();

    // 验证 request 被正确调用（路径为 /papers/types）
    expect(requestMocks.mockRequest).toHaveBeenCalledWith('/papers/types');
    // 验证返回值与预期一致
    expect(result).toEqual(expectedTypes);
  });
});

// ============================================================================
// translatePaper 测试
// ============================================================================

describe('translatePaper — 翻译论文标题和摘要', () => {
  it('发送 POST 请求触发翻译', async () => {
    const expectedData = {
      status: 'done',
      title_zh: '测试论文',
      abstract_zh: '这是摘要的中文翻译',
    };
    requestMocks.mockRequest.mockResolvedValueOnce(expectedData);

    const result = await translatePaper('1');

    expect(requestMocks.mockRequest).toHaveBeenCalledTimes(1);
    expect(requestMocks.mockRequest.mock.calls[0][0]).toBe('/papers/1/translate');
    expect(requestMocks.mockRequest.mock.calls[0][1].method).toBe('POST');
    expect(result).toEqual(expectedData);
  });

  it('返回 generating 状态表示翻译中', async () => {
    requestMocks.mockRequest.mockResolvedValueOnce({ status: 'generating' });

    const result = await translatePaper('1');

    expect(result.status).toBe('generating');
  });

  it('返回 done 状态表示翻译完成', async () => {
    requestMocks.mockRequest.mockResolvedValueOnce({
      status: 'done',
      title_zh: '中文标题',
      abstract_zh: '中文摘要',
    });

    const result = await translatePaper('1');

    expect(result.status).toBe('done');
    expect(result.title_zh).toBe('中文标题');
  });
});
