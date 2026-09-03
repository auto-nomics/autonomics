/**
 * Settings API 测试
 *
 * 测试设置相关的后端接口调用，包括：
 * - 获取所有用户设置
 * - 更新用户设置（支持部分更新）
 * - 获取 Token 预算配置
 */

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import type { Settings, TokenBudgets } from '@/types';
import {
  getSettings,
  updateSettings,
  getTokenBudgets,
} from '../settingsApi';

// 使用工厂函数创建 mock，避免变量提升问题
const requestMocks = vi.hoisted(() => ({
  mockRequest: vi.fn(),
}));

vi.mock('../client', () => ({
  default: requestMocks.mockRequest,
}));

// ============================================================================
// 测试环境设置
// ============================================================================

// beforeEach 钩子：每个测试前重置 mock
beforeEach(() => {
  requestMocks.mockRequest.mockClear();
  requestMocks.mockRequest.mockResolvedValue({});
});

// 重置全局 mock
afterEach(() => {
  vi.restoreAllMocks();
});

// ============================================================================
// getSettings 测试
// ============================================================================

describe('getSettings — 获取所有用户设置', () => {
  it('发送 GET 请求获取所有用户设置', async () => {
    const expectedSettings = {
      model_provider: 'openai',
      api_key: 'sk-test123',
      temperature: 0.7,
      parse_engine: 'mineru',
    };
    requestMocks.mockRequest.mockResolvedValueOnce(expectedSettings);

    const result = await getSettings();

    expect(requestMocks.mockRequest).toHaveBeenCalledTimes(1);
    // GET 带 ?_t= 时间戳防 Tauri WebView 缓存（d2a360f1），只断言路径主体
    expect(requestMocks.mockRequest.mock.calls[0][0]).toMatch(/^\/settings\?_t=\d+$/);
    expect(result).toEqual(expectedSettings);
  });

  it('返回键值对格式的设置数据', async () => {
    requestMocks.mockRequest.mockResolvedValueOnce({
      model_provider: 'anthropic',
      max_tokens: 4000,
      theme: 'dark',
    });

    const result = await getSettings() as unknown as Settings;

    expect(typeof result).toBe('object');
    expect(result.model_provider).toBe('anthropic');
  });

  it('返回空对象当用户没有设置时', async () => {
    requestMocks.mockRequest.mockResolvedValueOnce({});

    const result = await getSettings();

    expect(result).toEqual({});
  });
});

// ============================================================================
// updateSettings 测试
// ============================================================================

describe('updateSettings — 更新用户设置', () => {
  it('发送 PUT 请求更新用户设置', async () => {
    const newSettings = { model_provider: 'claude' } as any;
    requestMocks.mockRequest.mockResolvedValueOnce({ message: 'Settings updated' });

    await updateSettings(newSettings);

    expect(requestMocks.mockRequest).toHaveBeenCalledTimes(1);
    expect(requestMocks.mockRequest.mock.calls[0][0]).toBe('/settings');
    expect(requestMocks.mockRequest.mock.calls[0][1].method).toBe('PUT');
  });

  it('请求体包含在 settings 字段中', async () => {
    const newSettings = { temperature: 0.5, max_tokens: 2000 } as any;
    requestMocks.mockRequest.mockResolvedValueOnce({ message: 'Settings updated' });

    await updateSettings(newSettings);

    expect(requestMocks.mockRequest.mock.calls[0][1].body).toEqual({ settings: newSettings });
  });

  it('支持部分更新（只传需要修改的字段）', async () => {
    const partialUpdate = { model_provider: 'openai' } as any;
    requestMocks.mockRequest.mockResolvedValueOnce({ message: 'Settings updated' });

    await updateSettings(partialUpdate);

    expect(requestMocks.mockRequest.mock.calls[0][1].body).toEqual({ settings: partialUpdate });
  });

  it('支持批量更新多个设置项', async () => {
    const batchUpdate = {
      model_provider: 'anthropic',
      temperature: 0.8,
      max_tokens: 8000,
      parse_engine: 'mineru',
    } as any;
    requestMocks.mockRequest.mockResolvedValueOnce({ message: 'Settings updated' });

    await updateSettings(batchUpdate);

    expect(requestMocks.mockRequest.mock.calls[0][1].body).toEqual({ settings: batchUpdate });
  });

  it('返回更新确认消息', async () => {
    const expectedMessage = '设置已更新';
    requestMocks.mockRequest.mockResolvedValueOnce({ message: expectedMessage });

    const result = await updateSettings({ theme: 'light' } as any) as unknown as { message: string };

    expect(result.message).toBeTruthy();
  });
});

// ============================================================================
// getTokenBudgets 测试
// ============================================================================

describe('getTokenBudgets — 获取 Token 预算配置', () => {
  it('发送 GET 请求获取各模型的 Token 预算', async () => {
    const expectedBudgets = {
      'gpt-4o': 100000,
      'claude-3.5-sonnet': 180000,
      'claude-3.5-haiku': 200000,
    };
    requestMocks.mockRequest.mockResolvedValueOnce(expectedBudgets);

    const result = await getTokenBudgets();

    expect(requestMocks.mockRequest).toHaveBeenCalledTimes(1);
    expect(requestMocks.mockRequest.mock.calls[0][0]).toBe('/settings/token-budgets');
    expect(result).toEqual(expectedBudgets);
  });

  it('返回的预算值是数字类型', async () => {
    requestMocks.mockRequest.mockResolvedValueOnce({
      'gpt-4o': 100000,
      'claude-3.5-sonnet': 180000,
    });

    const result = await getTokenBudgets() as unknown as Record<string, number>;

    for (const model in result) {
      expect(typeof result[model]).toBe('number');
    }
  });

  it('返回包含多个模型配置的对象', async () => {
    requestMocks.mockRequest.mockResolvedValueOnce({
      'gpt-4o': 100000,
      'gpt-4o-mini': 50000,
      'claude-3.5-sonnet': 180000,
      'claude-3.5-haiku': 200000,
      'claude-3-opus': 150000,
    });

    const result = await getTokenBudgets() as unknown as Record<string, number>;

    expect(Object.keys(result).length).toBeGreaterThan(1);
  });
});
