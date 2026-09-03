/**
 * autonomics 通用 API 客户端
 *
 * 纯 fetch 实现，基础前缀 `/api/v1/bib`（autonomics 后端唯一的 API 命名空间）。
 *
 * 保留 jayread 客户端的全部横切能力：
 * - 响应缓存 + TTL 分类（papers 2s / categories 5s / translate 60s）
 * - GET 请求去重（相同 cacheKey 的并发请求共享同一个 Promise）
 * - invalidateCache(pattern) 手动失效
 * - 超时（AbortController）+ 网络错误重试
 * - 错误信封解析：autonomics 返回 `{"error": "..."}`，与原有的
 *   `errorBody?.error` 回落链天然兼容，toast 语义不变
 *
 * jayread 的 Tauri 双模式分支已删除：autonomics 只在浏览器里跑，恒走同源 fetch。
 * `isTauri` / `getTauriBaseUrl` 仍导出为常量 `false` / `''`，因为多个组件
 * （PaperReaderPage、WasmPdfViewer、usePanelCoordination）import 了它们做
 * 环境分支 —— 恒 false 让这些分支全部落到 web 路径，正是我们要的行为。
 */

/** 默认请求超时时间（毫秒） */
export const DEFAULT_TIMEOUT = 30000;

/** 默认重试次数（仅对网络错误重试） */
export const DEFAULT_RETRIES = 1;

/** autonomics 后端 API 基础前缀 */
export const API_BASE = '/api/v1/bib';

/**
 * 缓存 TTL 配置（毫秒）
 * - papers: 2s - 论文列表缓存，短时间避免频繁请求
 * - categories: 5s - 分类（autonomics collections）数据缓存
 * - translate: 60s - 翻译结果缓存（autonomics 无翻译能力，保留分类以兼容桩）
 * - none: 0 - 绝不缓存（见 getCacheCategory）
 */
const CACHE_TTL = {
  papers: 2000,
  categories: 5000,
  translate: 60000,
  none: 0,
  default: 1000,
} as const;

type CacheCategory = keyof typeof CACHE_TTL;

/**
 * 响应缓存条目
 */
interface CacheEntry<T = unknown> {
  data: T;
  timestamp: number;
  ttl: number;
}

/**
 * 待处理请求
 */
interface PendingRequest<T = unknown> {
  promise: Promise<T>;
  timestamp: number;
  controller?: AbortController;
}

// 响应缓存存储
const responseCache = new Map<string, CacheEntry>();

// 待处理请求存储（用于请求去重）
const pendingRequests = new Map<string, PendingRequest>();

/**
 * 生成缓存键
 */
function getCacheKey(path: string, options: Record<string, unknown>): string {
  const method = String(options.method || 'GET').toUpperCase();
  // 只缓存 GET 请求
  if (method !== 'GET') return '';

  const body = options.body;
  let bodyKey = '';
  if (body) {
    try {
      bodyKey = typeof body === 'string' ? body : JSON.stringify(body);
    } catch {
      bodyKey = String(body);
    }
  }
  return `${method}:${path}:${bodyKey}`;
}

/**
 * 获取缓存分类
 *
 * 注意：匹配的是 autonomics 的路径（/articles、/collections），
 * 不是 jayread 的 /papers、/categories。
 *
 * `/chat` 与 `/settings` 永不缓存（TTL 0）：两者都是读-改-写或读-显示最新值的
 * 端点，读到旧值会导致覆盖丢消息 / 模型配置丢失。jayread 靠 `_t` 时间戳绕开
 * 缓存，但 `Date.now()` 只有毫秒精度，同一毫秒内的两次请求仍会命中 —— 这里
 * 在 TTL 层面根治。
 */
function getCacheCategory(path: string): CacheCategory {
  if (path.includes('/chat') || path.includes('/settings')) return 'none';
  if (path.includes('/articles') || path.includes('/fulltext')) return 'papers';
  if (path.includes('/collections')) return 'categories';
  if (path.includes('/translate')) return 'translate';
  return 'default';
}

/**
 * 带缓存的请求包装器
 * - 检查缓存，如果有效则直接返回
 * - 检查待处理请求，如果存在则返回相同 Promise（请求去重）
 * - 发起新请求并缓存结果
 */
async function cachedRequest<T = unknown>(
  path: string,
  options: Record<string, unknown>,
  requester: (p: string, o: Record<string, unknown>) => Promise<T>
): Promise<T> {
  const cacheKey = getCacheKey(path, options);
  const category = getCacheCategory(path);
  const ttl = CACHE_TTL[category];

  // 检查缓存
  if (cacheKey) {
    const cached = responseCache.get(cacheKey);
    if (cached && Date.now() - cached.timestamp < cached.ttl) {
      return cached.data as T;
    }
  }

  // 检查待处理请求（去重）
  if (cacheKey && pendingRequests.has(cacheKey)) {
    const pending = pendingRequests.get(cacheKey)!;
    // 清理超过 30 秒的待处理请求（防止内存泄漏）
    if (Date.now() - pending.timestamp < 30000) {
      return pending.promise as Promise<T>;
    } else {
      // Abort the controller before removing the expired request
      if (pending.controller) {
        pending.controller.abort();
      }
      pendingRequests.delete(cacheKey);
    }
  }

  // 发起新请求
  // Create a controller for this request so we can abort it if needed
  const controller = new AbortController();
  // Pass the signal to the requester (if no external signal, use our controller)
  const requestOptions = options.signal
    ? options
    : { ...options, signal: controller.signal };

  const requestPromise = requester(path, requestOptions);

  // 记录待处理请求
  if (cacheKey) {
    pendingRequests.set(cacheKey, {
      promise: requestPromise,
      timestamp: Date.now(),
      controller,
    });
  }

  try {
    const result = await requestPromise;

    // 缓存结果（仅成功时）
    if (cacheKey && result !== undefined) {
      responseCache.set(cacheKey, {
        data: result,
        timestamp: Date.now(),
        ttl,
      });
    }

    return result;
  } finally {
    // 清理待处理请求
    if (cacheKey) {
      pendingRequests.delete(cacheKey);
    }
  }
}

/**
 * 使缓存失效
 * @param pattern - 路径模式，支持部分匹配。例如：'/articles' 会清除所有包含 '/articles' 的缓存
 */
export function invalidateCache(pattern?: string): void {
  if (!pattern) {
    // 清除所有缓存
    responseCache.clear();
    pendingRequests.clear();
    return;
  }

  // 清除匹配模式的缓存
  for (const key of responseCache.keys()) {
    if (key.includes(pattern)) {
      responseCache.delete(key);
    }
  }

  // 清除匹配模式的待处理请求
  for (const key of pendingRequests.keys()) {
    if (key.includes(pattern)) {
      const entry = pendingRequests.get(key);
      if (entry?.controller) {
        entry.controller.abort();
      }
      pendingRequests.delete(key);
    }
  }
}

// ============================================================
// 环境标记（兼容导出：autonomics 只有 web 模式）
// ============================================================

/** 恒为 false：autonomics 前端只以浏览器模式运行（无 Tauri 壳）。 */
export const isTauri = false;
/** 同 isTauri，保留导出名以兼容既有 import。 */
export const isDesktop = false;

/**
 * 兼容导出：jayread 在 Tauri 模式下返回 sidecar 地址用于拼接绝对 URL。
 * autonomics 恒为同源，返回空串即可（`base + path` 拼接不受影响）。
 */
export async function getTauriBaseUrl(): Promise<string> {
  return '';
}

// ============================================================
// 鉴权头
// ============================================================

/**
 * 读可选的 bearer token。
 *
 * autonomics 的 tui-http 有 bearer 认证层（只拦 /api/）。本地部署通常不设 token，
 * 此时返回 null 且**不附带任何 Authorization 头**，请求行为与无鉴权完全一致。
 */
function getAuthToken(): string | null {
  try {
    const token = localStorage.getItem('autonomics_token');
    return token && token.trim() ? token.trim() : null;
  } catch {
    return null; // localStorage 不可访问（隐私模式等）
  }
}

/**
 * 请求实现（不包含缓存）
 */
async function rawRequest(requestPath: string, options: Record<string, unknown>): Promise<unknown> {
  const url = `${API_BASE}${requestPath}`;

  const { timeout = DEFAULT_TIMEOUT, retries = DEFAULT_RETRIES, signal: externalSignal, ...fetchOptions } = options;

  const headers: Record<string, string> = {
    ...((fetchOptions.headers as Record<string, string>) || {}),
  };

  const token = getAuthToken();
  if (token) {
    headers.Authorization = `Bearer ${token}`;
  }

  const config: Record<string, unknown> = {
    ...fetchOptions,
    headers,
  };

  const body = config.body;
  if (body && typeof body !== 'string' && !(body instanceof FormData) && !headers['Content-Type']) {
    headers['Content-Type'] = 'application/json';
    config.body = JSON.stringify(body);
  }

  let lastError: Error | undefined;
  for (let attempt = 0; attempt <= (retries as number); attempt++) {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), timeout as number);

    if (externalSignal) {
      (externalSignal as AbortSignal).addEventListener('abort', () => controller.abort(), { once: true });
      if ((externalSignal as AbortSignal).aborted) {
        controller.abort();
      }
    }

    config.signal = controller.signal;

    try {
      const response = await fetch(url, config as RequestInit);
      clearTimeout(timeoutId);

      if (!response.ok) {
        // autonomics 错误信封：{"error": "..."}；
        // message / detail 两个回落键保留给非标准响应（例如中间代理）。
        const errorBody = await response.json().catch(() => null);
        const detail = errorBody?.error || errorBody?.message || errorBody?.detail || response.statusText;
        throw new Error(typeof detail === 'string' && detail ? detail : `请求失败: ${response.status}`);
      }

      if (response.status === 204 || response.headers.get('content-length') === '0') {
        return null;
      }
      return response.json();
    } catch (err) {
      clearTimeout(timeoutId);

      if ((err as Error).name === 'AbortError') {
        if (externalSignal && (externalSignal as AbortSignal).aborted) {
          throw err;
        }
        throw new Error(`请求超时（${(timeout as number) / 1000}秒），请检查网络连接或稍后重试`);
      }

      if (!(err instanceof TypeError)) {
        throw err;
      }

      lastError = err as Error;
      if (attempt < (retries as number)) {
        await new Promise(r => setTimeout(r, 500));
      }
    }
  }

  throw new Error(`网络请求失败（已重试 ${retries} 次）：${lastError?.message}`);
}

/**
 * 统一请求入口（带缓存）
 *
 * @param path - 不含 `/api/v1/bib` 前缀的路径，如 `/articles`、`/collections`
 */
async function request<T = unknown>(path: string | Record<string, unknown>, options: Record<string, unknown> = {}): Promise<T> {
  // Handle legacy signature where first arg might be path string
  const requestPath = typeof path === 'string' ? path : '';
  if (!requestPath && typeof path !== 'string') {
    options = path;
  }

  const requester: (p: string, o: Record<string, unknown>) => Promise<T> =
    rawRequest as typeof requester;

  // 使用缓存包装器
  return cachedRequest<T>(requestPath, options, requester);
}

export default request;
