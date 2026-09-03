/**
 * 本地文件搜索 API 客户端
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * jayread 在 Tauri 桌面模式下让用户按关键词搜索本机文件再作为聊天附件。
 * autonomics 是纯浏览器部署，既没有文件系统访问能力，也没有对应端点。
 *
 * 返回空结果集（而不是 reject）：文件搜索弹窗渲染为「无匹配」，是这类
 * 搜索 UI 最自然的空态。
 *
 * 导出签名与 jayread 版本一致；UI 入口隐藏属 Phase 5 清扫。
 */

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
 * ⚠️ 桩：autonomics 无本地文件系统访问能力，恒返回空结果。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * @param {string} query - 搜索关键词
 * @param {{ limit?: number; root?: string }} [options] - 可选配置（兼容签名，未使用）
 * @returns {Promise<{ results: FileSearchResult[] }>} 搜索结果列表
 */
export async function searchFiles(
  _query: string,
  _options?: { limit?: number; root?: string }
): Promise<{ results: FileSearchResult[] }> {
  return { results: [] };
}
