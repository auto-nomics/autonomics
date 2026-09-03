/**
 * JayRead 网络搜索 API 客户端
 *
 * 封装基于 Tavily API 的网络搜索接口，供 ChatPanel 在发送 AI 消息前调用。
 * 搜索结果会注入到 AI 对话的 system prompt 中（prompt injection 模式）。
 *
 * 工作流程：
 * 1. 用户在 AI 对话面板点击地球图标开启网络搜索
 * 2. 用户发送消息时，如果网络搜索已开启，先调用 webSearch() 获取搜索结果
 * 3. 后端从 .env 环境变量读取 Tavily API Key，调用 Tavily 搜索 API
 * 4. 返回的搜索结果（formatted_prompt）被注入到 system prompt 中
 * 5. AI 模型在生成回复时会参考这些搜索结果
 *
 * 为什么搜索 API 要走后端而不是前端直接调用 Tavily？
 * - Tavily API Key 不应暴露在浏览器端（安全风险）
 * - 后端统一管理 API Key 和请求频率
 * - 与现有的 AI 代理模式一致（后端作为中间人转发请求）
 */

// 导入通用 API 客户端封装函数
// request() 会自动添加 /api 前缀、设置 Content-Type、序列化请求体
import request from './client';
import type { WebSearchAPIResponse, CheckTavilyKeyResponse } from '@/types';

/**
 * 执行网络搜索
 *
 * 调用后端 /api/search/web 接口，使用 Tavily API 搜索与用户查询相关的网络内容。
 * 搜索结果包含格式化的 Markdown 文本（formatted_prompt），可直接注入 system prompt。
 *
 * 使用场景：
 * - 用户开启网络搜索开关后发送消息时
 * - 在 ChatPanel 的 handleSend 函数中调用
 *
 * 错误处理：
 * - API Key 未配置时，后端返回 400 错误，前端会提示用户配置
 * - 搜索超时或失败时，后端返回空结果和错误信息
 * - 网络错误时，request() 会抛出异常，调用方需要 try/catch
 *
 * @param {string} query - 搜索查询文本（通常是用户发送的消息内容）
 * @returns {Promise<WebSearchAPIResponse>} 搜索结果对象
 */
export async function webSearch(query: string): Promise<WebSearchAPIResponse> {
  // 调用后端 POST /api/search/web 接口
  // request() 会自动拼接为 /api/search/web 并序列化请求体
  return request<WebSearchAPIResponse>('/search/web', {
    method: 'POST',        // HTTP 方法：POST（提交搜索查询）
    body: { query },       // 请求体：包含搜索查询文本
  });
}

/**
 * 检查 Tavily API Key 状态
 *
 * 调用后端 GET /api/search/check 接口，验证 Tavily API Key 是否已配置且可用。
 * 在用户开启 Web Search 开关时作为预检查调用。
 *
 * @returns {Promise<CheckTavilyKeyResponse>} 检查结果对象
 */
export async function checkTavilyKey(): Promise<CheckTavilyKeyResponse> {
  return request<CheckTavilyKeyResponse>('/search/check', {
    method: 'GET',
  });
}
