/**
 * JayRead 本地文件搜索 API 客户端
 *
 * 封装本地文件系统搜索接口：
 * - 按关键词搜索文件
 * - 支持限制结果数量
 * - 支持指定搜索根目录
 *
 * 使用场景：
 * - 用户在论文上传界面选择本地文件
 * - 通过关键词快速定位文件
 * - 支持搜索文档目录等
 */
import request from './client';

/**
 * 文件搜索结果
 */
export interface FileSearchResult {
  /** 文件完整路径 */
  path: string;
  /** 文件名（不含路径） */
  name: string;
  /** 是否为目录（前端按文件类型上色用） */
  is_directory: boolean;
}

/**
 * 搜索本地文件
 *
 * @param {string} query - 搜索关键词
 * @param {{ limit?: number; root?: string }} [options] - 可选配置
 * @param {number} [options.limit=50] - 返回结果数量限制
 * @param {string} [options.root='~'] - 搜索根目录，默认为用户主目录
 * @returns {Promise<{ results: FileSearchResult[] }>} 搜索结果列表
 */
export async function searchFiles(
  query: string,
  options?: { limit?: number; root?: string }
): Promise<{ results: FileSearchResult[] }> {
  const params = new URLSearchParams({
    q: query,
    limit: String(options?.limit ?? 50),
    root: options?.root ?? '~',
  });
  const data = await request(`/files/search?${params.toString()}`);
  // 后端直接返回数组，前端包装为统一格式
  if (Array.isArray(data)) {
    return { results: data };
  }
  return data as { results: FileSearchResult[] };
}
