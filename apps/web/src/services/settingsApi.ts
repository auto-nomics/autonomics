/**
 * JayRead 设置相关 API 客户端
 *
 * 封装用户配置和系统设置的持久化接口：
 * - AI 模型选择（OpenAI / Anthropic / 本地模型）
 * - API 密钥配置
 * - 解析引擎偏好设置
 * - 其他用户自定义选项
 *
 * 为什么使用键值对存储而不是固定的 schema？
 * - 便于动态添加新的设置项，无需修改数据库结构
 * - 不同用户的配置差异较大，键值对更灵活
 * - 前端可以灵活地组织设置 UI，无需与后端 schema 强耦合
 *
 * 数据格式说明：
 * - 后端存储为 JSON 键值对，如 { "model_provider": "openai", "api_key": "sk-xxx" }
 * - 前端可以根据需要将设置分组展示
 */
import request from './client'; // 导入通用 API 客户端封装函数
import type {
  GetSettingsResponse,
  UpdateSettingsRequest,
  UpdateSettingsResponse,
  GetTokenBudgetsResponse,
} from '@/types';

/**
 * 获取当前用户的所有设置项
 *
 * 使用场景：
 * - 应用启动时加载用户配置
 * - 设置页面打开时显示当前值
 * - 切换 AI 模型前检查当前配置
 *
 * 为什么一次性返回所有设置而不是分页？
 * - 设置项数量有限（通常几十个以内）
 * - 减少网络请求次数，提升用户体验
 * - 便于设置页面的一次性渲染
 *
 * @param {AbortSignal} [signal] - 可选的 AbortSignal，用于取消请求
 * @returns {Promise<GetSettingsResponse>} 键值对字典
 */
export async function getSettings(signal?: AbortSignal): Promise<GetSettingsResponse> { // 导出获取设置函数
  // 加 cache-bust query: Tauri WebView (webkit2gtk / webview2 / wkwebview) 在某些
  // 情况下会无视 fetch 的 `cache: 'no-store'` 和后端的 Cache-Control header,
  // 把 GET /settings 缓存到磁盘。重启后读到旧值,model 配置会丢失甚至被覆盖。
  // 让每次请求 URL 都不同,从 URL 层强制走网络。
  return request<GetSettingsResponse>(`/settings?_t=${Date.now()}`, { signal }); // GET 请求获取所有用户设置
}

/**
 * 更新用户设置（支持部分更新）
 *
 * 为什么使用 PUT 而不是 POST？
 * - PUT 语义是"替换/更新资源"，更符合"更新设置"的含义
 * - RESTful 规范：PUT 用于更新，POST 用于创建
 * - 虽然 RESTful 也允许 POST 用于部分更新，但 PUT 更明确
 *
 * 为什么支持部分更新而不是必须发送完整设置？
 * - 用户可能只修改单个设置项（如切换模型）
 * - 减少网络传输数据量
 * - 避免并发修改时的数据覆盖问题
 *
 * @param {UpdateSettingsRequest} settings - 要更新的设置键值对（Partial<Settings>，无需外层包装）
 * @returns {Promise<UpdateSettingsResponse>} 更新确认消息
 */
export async function updateSettings(settings: UpdateSettingsRequest): Promise<UpdateSettingsResponse> {
  return request<UpdateSettingsResponse>('/settings', {
    method: 'PUT',
    body: { settings }, // 后端契约：键值对需包裹在 settings 字段中
  });
}

/**
 * 获取各 AI 模型的 Token 预算配置
 *
 * 使用场景：
 * - ChatPanel 组件初始化时加载预算配置
 * - 用于决定注入多少上下文到 AI 对话中
 * - 前端根据预算动态调整内容分块策略
 *
 * 为什么需要这个 API？
 * - 不同模型的 context window 差异巨大（4K - 1M+）
 * - 后端可以根据最新模型信息更新预算，前端无需重新部署
 * - 支持用户自定义预算配置（未来功能）
 *
 * 优化项 2 改进（2026-04）:
 * - API 响应格式从 { "gpt-4o": 100000, ... } 变为 { budgets: {...}, default_budget: 150000 }
 * - 前端使用 tokenBudgets.budgets?.[model] || tokenBudgets.default_budget || 150000 做精确匹配 + 回退
 * - 扩展新模型只需改后端 config.py，前端零改动
 *
 * @returns {Promise<GetTokenBudgetsResponse>} Token 预算配置对象
 */
export async function getTokenBudgets(): Promise<GetTokenBudgetsResponse> { // 导出获取 token 预算函数
  return request<GetTokenBudgetsResponse>('/settings/token-budgets'); // GET 请求获取 token 预算配置
}
