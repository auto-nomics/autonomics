/** 默认请求超时时间（毫秒） */
export const DEFAULT_TIMEOUT = 30000;

/** 默认重试次数（仅对网络错误重试） */
export const DEFAULT_RETRIES = 1;

/**
 * 缓存 TTL 配置（毫秒）
 * - papers: 2s - 论文列表缓存，短时间避免频繁请求
 * - categories: 5s - 分类数据缓存
 * - translate: 60s - 翻译结果缓存，避免重复翻译相同内容
 */
const CACHE_TTL = {
  papers: 2000,
  categories: 5000,
  translate: 60000,
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
 */
function getCacheCategory(path: string): CacheCategory {
  if (path.includes('/papers') || path.includes('/paper')) return 'papers';
  if (path.includes('/categories')) return 'categories';
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
 * @param pattern - 路径模式，支持部分匹配。例如：'/papers' 会清除所有包含 '/papers' 的缓存
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

/**
 * JayRead 通用 API 客户端
 *
 * 双模式支持：
 * - Web 模式：使用 fetch 发送请求到 /api 前缀
 * - Tauri 桌面模式：通过 Tauri Rust 后端代理请求到 sidecar Fastify 服务器
 *
 * 自动检测运行环境并选择对应的请求方式。
 */

const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
const isDesktop = isTauri;

let _tauriBaseUrl: string | null = null;

// Server readiness state (Tauri mode only)
let _serverReady = false;
let _serverReadyPromise: Promise<void> | null = null;

async function waitForServer(baseUrl: string): Promise<void> {
  if (_serverReady) return;
  if (_serverReadyPromise) return _serverReadyPromise;

  _serverReadyPromise = (async () => {
    const maxAttempts = 40; // 40 * 1.5s = 60s max wait
    for (let i = 0; i < maxAttempts; i++) {
      try {
        const resp = await fetch(`${baseUrl}/api/health`, {
          signal: AbortSignal.timeout(2000),
        });
        if (resp.ok) {
          _serverReady = true;
          console.log(`[JayRead] Server ready after ${i + 1} attempts`);
          return;
        }
      } catch { /* server not up yet */ }
      await new Promise(r => setTimeout(r, 1500));
    }
    console.warn('[JayRead] Server did not become ready within timeout');
  })();

  return _serverReadyPromise;
}

async function getTauriBaseUrl(): Promise<string> {
  if (_tauriBaseUrl) return _tauriBaseUrl;

  try {
    const { invoke } = await import('@tauri-apps/api/core');
    _tauriBaseUrl = await invoke('get_sidecar_url');
  } catch {
    _tauriBaseUrl = 'http://localhost:3001';
  }
  return _tauriBaseUrl!;
}

/** Tauri 模式下通过 Rust 后端代理 API 请求 */
async function tauriRequest(path: string, options: Record<string, unknown> = {}): Promise<unknown> {
  const { invoke } = await import('@tauri-apps/api/core');
  const baseUrl = await getTauriBaseUrl();

  // Wait for server to be ready (polls /api/health)
  await waitForServer(baseUrl);

  const fullPath = `/api${path}`;

  const { timeout = DEFAULT_TIMEOUT, retries: _retries, signal: _signal, ...fetchOptions } = options;

  const tauriOptions: Record<string, unknown> = {
    method: (fetchOptions as Record<string, unknown>).method || 'GET',
    headers: (fetchOptions as Record<string, unknown>).headers || {},
    body: null as unknown,
    timeout: timeout as number,
  };

  const rawBody = (fetchOptions as Record<string, unknown>).body;
  const isFormData = rawBody instanceof FormData;
  if (rawBody && !isFormData) {
    tauriOptions.body = typeof rawBody === 'string' ? JSON.parse(rawBody) : rawBody;
  }

  try {
    // Try direct fetch to sidecar first (better SSE support)
    const fetchBody = isFormData
      ? rawBody
      : (tauriOptions.body ? JSON.stringify(tauriOptions.body) : undefined);
    const fetchHeaders: Record<string, string> = isFormData
      ? {}  // Let browser set Content-Type with boundary for multipart
      : {
          ...tauriOptions.headers as Record<string, string>,
          ...(fetchBody ? { 'Content-Type': 'application/json' } : {}),
        };

    const response = await fetch(`${baseUrl}${fullPath}`, {
      method: tauriOptions.method as string,
      headers: fetchHeaders,
      body: fetchBody,
      signal: _signal as AbortSignal,
      cache: 'no-store',
    });

    if (!response.ok) {
      const errorBody = await response.json().catch(() => null);
      const detail = errorBody?.message || errorBody?.detail || errorBody?.error || response.statusText;
      throw new Error(typeof detail === 'string' && detail ? detail : `请求失败: ${response.status}`);
    }

    if (response.status === 204 || response.headers.get('content-length') === '0') {
      return null;
    }

    return response.json();
  } catch (err) {
    if (err instanceof Error && err.message.includes('请求失败')) {
      throw err;
    }
    // Tauri mode: fallback to invoke if direct fetch fails
    try {
      // For FormData, serialize into multipart format that Tauri can forward
      if (isFormData && rawBody instanceof FormData) {
        const fields = [];
        for (const [name, value] of rawBody.entries()) {
          if (value instanceof File) {
            const buffer = await value.arrayBuffer();
            const bytes = new Uint8Array(buffer);
            let binary = '';
            for (let i = 0; i < bytes.length; i++) {
              binary += String.fromCharCode(bytes[i]);
            }
            fields.push({
              name,
              file_data: btoa(binary),
              file_name: value.name,
              content_type: value.type || 'application/octet-stream',
            });
          } else {
            fields.push({ name, value: String(value) });
          }
        }
        tauriOptions.multipart = { fields };
        tauriOptions.body = null;
      }
      return await invoke('api_request', { path: fullPath, options: tauriOptions });
    } catch (invokeErr) {
      const msg = typeof invokeErr === 'string' ? invokeErr
        : (invokeErr instanceof Error ? invokeErr.message : String(invokeErr));
      throw new Error(msg || '请求失败');
    }
  }
}

/**
 * Web 模式请求实现（不包含缓存）
 */
async function webRequest(requestPath: string, options: Record<string, unknown>): Promise<unknown> {
  const url = `/api${requestPath}`;

  const { timeout = DEFAULT_TIMEOUT, retries = DEFAULT_RETRIES, signal: externalSignal, ...fetchOptions } = options;

  const config: Record<string, unknown> = {
    headers: {},
    ...fetchOptions,
  };

  const body = config.body;
  if (body && typeof body !== 'string' && !(body instanceof FormData) && !(config.headers as Record<string, unknown>)['Content-Type']) {
    (config.headers as Record<string, unknown>)['Content-Type'] = 'application/json';
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
        const errorBody = await response.json().catch(() => null);
        const detail = errorBody?.message || errorBody?.detail || errorBody?.error || response.statusText;
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
 */
async function request<T = unknown>(path: string | Record<string, unknown>, options: Record<string, unknown> = {}): Promise<T> {
  // Handle legacy signature where first arg might be path string
  const requestPath = typeof path === 'string' ? path : '';
  if (!requestPath && typeof path !== 'string') {
    options = path;
  }

  // 确定请求方式：CEF 和 Tauri 都通过本地 HTTP 代理，Web 直接请求
  const requester: (p: string, o: Record<string, unknown>) => Promise<T> =
    isDesktop ? tauriRequest as typeof requester : webRequest as typeof requester;

  // 使用缓存包装器
  return cachedRequest<T>(requestPath, options, requester);
}

export default request;

export { isTauri, isDesktop, getTauriBaseUrl };
