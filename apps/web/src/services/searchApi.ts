/**
 * 网络搜索 API 客户端
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * jayread 用后端代理的 Tavily 搜索把网络结果注入聊天 system prompt。
 * autonomics 的聊天走 agentik 工具集（含 `lit_search` 文献检索工具，plan §4.2），
 * 通用 Web 搜索不在后端能力清单里，函数桩化。
 *
 * 降级策略：
 * - `webSearch` reject：调用方（ChatPanel 发送前）catch 后照常发消息，只是
 *   没有网络结果注入
 * - `checkTavilyKey` 返回 `{configured: false, valid: false}`：联网搜索开关
 *   显示「未配置」，用户不会打开一个注定失败的功能
 *
 * 导出签名与 jayread 版本一致；UI 入口隐藏属 Phase 5 清扫。
 */
import type { WebSearchAPIResponse, CheckTavilyKeyResponse } from '@/types';

/**
 * 执行网络搜索
 *
 * ⚠️ 桩：autonomics 无 Web 搜索后端。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * @param {string} query - 搜索查询文本
 * @returns {Promise<WebSearchAPIResponse>} 搜索结果对象（本实现恒 reject）
 */
export async function webSearch(query: string): Promise<WebSearchAPIResponse> {
  throw new Error(`网络搜索暂不可用（autonomics 无搜索后端），查询：${query.slice(0, 40)}`);
}

/**
 * 检查网络搜索后端是否可用
 *
 * ⚠️ 桩：恒返回未配置。UI 据此禁用联网搜索开关。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * @returns {Promise<CheckTavilyKeyResponse>} 检查结果对象
 */
export async function checkTavilyKey(): Promise<CheckTavilyKeyResponse> {
  return {
    configured: false,
    valid: false,
    error: 'autonomics 未提供网络搜索能力',
  };
}
