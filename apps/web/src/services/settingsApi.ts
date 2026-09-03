/**
 * 设置相关 API 客户端（autonomics settings）
 *
 * 后端契约：`GET /settings` → `{"settings": {...}}`，`PUT /settings` body
 * `{"settings": {...}}`。autonomics 用 bib_meta KV 存储（键名带 `web:` 前缀
 * 命名空间，GET 聚合时剥前缀），整体读写、逐键 upsert 语义由后端负责。
 *
 * 前端侧完全不用关心前缀：这里只做信封拆装，键集合是开放的
 * （custom_model_configs / web_search_enabled / custom_prompt_templates ...），
 * 后端对未知键照单全收。
 */
import request from './client';
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
 * - 应用启动时加载用户配置（useChatInit 的 getSettingsWithRetry）
 * - 设置页面打开时显示当前值
 *
 * @param {AbortSignal} [signal] - 可选的 AbortSignal，用于取消请求
 * @returns {Promise<GetSettingsResponse>} 键值对字典
 */
export async function getSettings(signal?: AbortSignal): Promise<GetSettingsResponse> {
  // 加 cache-bust query: WebView (webkit2gtk / webview2 / wkwebview) 在某些情况下
  // 会无视 fetch 的 `cache: 'no-store'` 把 GET /settings 缓存到磁盘。重启后读到
  // 旧值，model 配置会丢失甚至被覆盖。让每次请求 URL 都不同，从 URL 层强制走网络。
  // （同时绕过 client 自身的 GET 缓存 —— 设置必须是最新值。）
  return request<GetSettingsResponse>(`/settings?_t=${Date.now()}`, { signal });
}

/**
 * 更新用户设置（支持部分更新）
 *
 * 后端做逐键 upsert，所以只传需要改的键即可；整体信封 `{settings: {...}}`
 * 在这里包装。
 *
 * @param {UpdateSettingsRequest} settings - 要更新的设置键值对（无需外层包装）
 * @returns {Promise<UpdateSettingsResponse>} 更新确认消息
 */
export async function updateSettings(settings: UpdateSettingsRequest): Promise<UpdateSettingsResponse> {
  // 后端做逐键 upsert、返回 {"ok":true}；省略的键保持原值，所以多标签页各写
  // 各的切片不会互相覆盖
  await request<{ ok?: boolean }>('/settings', {
    method: 'PUT',
    body: { settings }, // 后端契约：键值对需包裹在 settings 字段中
  });
  // jayread 调用方只关心成功与否，统一折算成消息形状
  return { message: '已保存' };
}

/**
 * 获取各 AI 模型的 Token 预算配置
 *
 * ⚠️ 桩：autonomics 的模型归服务端所有（apps/tui config.db），API key 永不
 * 进浏览器，也就没有「按模型动态预算」这回事。返回空对象。
 *
 * 当前无线上调用方（应用实际用的是 chatConstants.ts 的 DEFAULT_TOKEN_BUDGETS
 * 常量，且每个消费点都有 `|| 150000` 兜底），保留导出以对齐签名。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function getTokenBudgets(): Promise<GetTokenBudgetsResponse> {
  return {};
}
