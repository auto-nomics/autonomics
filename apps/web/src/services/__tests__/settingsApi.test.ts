/**
 * Settings API 测试（autonomics /settings 契约）
 *
 * - GET  /settings → {"settings": {...}}
 * - PUT  /settings body {"settings": {...}}（后端逐键 upsert）
 * - getTokenBudgets 桩化（autonomics 模型归服务端所有）
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { Settings } from '@/types';
import { getSettings, updateSettings, getTokenBudgets } from '../settingsApi';
import { invalidateCache } from '../client';

function jsonResponse(body: unknown, ok = true, status = 200) {
  return {
    ok,
    status,
    statusText: ok ? 'OK' : 'Error',
    headers: { get: () => 'application/json' },
    json: () => Promise.resolve(body),
  };
}

/** 带参数签名的 fetch mock，让 f.mock.calls 的解构有正确的元组类型 */
function makeFetch(impl?: (url: string, init?: RequestInit) => unknown) {
  const f = vi.fn(impl ?? ((url: string, init?: RequestInit) => jsonResponse({})));
  global.fetch = f as unknown as typeof fetch;
  return f;
}

beforeEach(() => {
  vi.clearAllMocks();
  invalidateCache();
});

afterEach(() => {
  vi.restoreAllMocks();
  invalidateCache();
});

// ============================================================================
// getSettings
// ============================================================================

describe('getSettings — 获取所有用户设置', () => {
  it('GET /settings，并解出 {settings} 信封', async () => {
    const settings = {
      model_provider: 'openai',
      custom_model_configs: '[]',
      web_search_enabled: 'false',
    };
    const f = makeFetch(() => Promise.resolve(jsonResponse({ settings })));

    const result = await getSettings();

    // GET 带 ?_t= 时间戳：绕开 WebView 磁盘缓存与 client 响应缓存
    const url = f.mock.calls[0][0] as string;
    expect(url).toMatch(/^\/api\/v1\/bib\/settings\?_t=\d+$/);
    expect(result).toEqual({ settings });
  });

  it('返回值直接可用作 settingsData.settings（useChatInit 的消费形状）', async () => {
    const settings = {
      selected_custom_model_config_id: 'cfg-1',
      custom_prompt_templates: '{"a":1}',
      model_config: '{"provider":"anthropic"}',
    } as unknown as Settings;
    makeFetch(() => Promise.resolve(jsonResponse({ settings })));

    const { settings: out } = (await getSettings()) as unknown as { settings: Record<string, unknown> };
    expect(out.selected_custom_model_config_id).toBe('cfg-1');
    expect(out.custom_prompt_templates).toBe('{"a":1}');
  });

  it('autonomics 键集合是开放的（未知键照常往返）', async () => {
    const settings = { some_future_key: 'value' } as unknown as Settings;
    global.fetch = vi.fn(() => Promise.resolve(jsonResponse({ settings }))) as unknown as typeof fetch;

    const { settings: out } = await getSettings();
    expect(out).toEqual({ some_future_key: 'value' });
  });
});

// ============================================================================
// updateSettings
// ============================================================================

describe('updateSettings — 更新用户设置', () => {
  it('PUT /settings，body 包裹在 settings 字段中', async () => {
    const f = makeFetch(() => Promise.resolve(jsonResponse({ settings: {} })));

    await updateSettings({ model_provider: 'claude' } as never);

    const [url, init] = f.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/v1/bib/settings');
    expect(init.method).toBe('PUT');
    expect(JSON.parse(init.body as string)).toEqual({ settings: { model_provider: 'claude' } });
  });

  it('支持部分更新（只传需要修改的键，后端逐键 upsert）', async () => {
    const f = makeFetch(() => Promise.resolve(jsonResponse({ settings: {} })));

    await updateSettings({ web_search_enabled: 'true' } as never);

    const [, init] = f.mock.calls[0] as [string, RequestInit];
    expect(JSON.parse(init.body as string)).toEqual({ settings: { web_search_enabled: 'true' } });
  });

  it('返回 {message} 确认形状（调用方只关心成功与否）', async () => {
    makeFetch(() => Promise.resolve(jsonResponse({ settings: {} })));

    const res = await updateSettings({} as never);
    expect(res.message).toBeTruthy();
  });
});

// ============================================================================
// getTokenBudgets（桩）
// ============================================================================

describe('getTokenBudgets — 桩', () => {
  it('返回空对象且不发网络请求（消费方有 || 150000 兜底）', async () => {
    const f = vi.fn(() => Promise.resolve(jsonResponse({})));
    global.fetch = f as unknown as typeof fetch;

    const res = await getTokenBudgets();

    expect(res).toEqual({});
    expect(f).not.toHaveBeenCalled();
  });
});
