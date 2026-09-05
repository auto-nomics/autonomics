/**
 * agentThreadsApi 单元测试
 *
 * 覆盖三组契约：
 * 1. 映射键归一：真实 ID / safeId / undefined 三种输入收敛到同一键，
 *    agentType 参与键（reader 与 screening 不串会话）
 * 2. 模式探测降级：任何失败（非 JSON / 无 mode / 网络错）都落 'ephemeral'，
 *    desktop 与旧后端因此永远走旧端点
 * 3. ensureMainThreadSession 并发去重：同键并发只发一次创建请求
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  __setAgentModeForTests,
  probeAgentMode,
  threadMappingKey,
  getStoredThreadId,
  setStoredThreadId,
  clearStoredThreadId,
  ensureMainThreadSession,
} from '../agentThreadsApi';

// happy-dom 暴露的 localStorage 缺方法（client.test.ts 同款问题）：内存 shim
const storageStore = new Map<string, string>();
const localStorageShim = {
  getItem: (k: string) => (storageStore.has(k) ? (storageStore.get(k) as string) : null),
  setItem: (k: string, v: string) => { storageStore.set(k, String(v)); },
  removeItem: (k: string) => { storageStore.delete(k); },
  clear: () => storageStore.clear(),
};

describe('agentThreadsApi', () => {
  beforeEach(() => {
    globalThis.localStorage = localStorageShim as unknown as Storage;
    storageStore.clear();
    __setAgentModeForTests(null);
    vi.stubGlobal('fetch', vi.fn());
  });

  afterEach(() => {
    __setAgentModeForTests(null);
    vi.unstubAllGlobals();
  });

  describe('threadMappingKey：scope + agentType 二元组', () => {
    it('null/undefined 归 standalone scope', () => {
      expect(threadMappingKey(null, 'homepage')).toBe(threadMappingKey(undefined, 'homepage'));
      expect(threadMappingKey(null, 'homepage')).toContain('standalone|homepage');
    });

    it('真实 ID 与字符串数字 ID 收敛到同一键', () => {
      expect(threadMappingKey(42, 'paperReader')).toBe(threadMappingKey('42', 'paperReader'));
    });

    it('不同 agentType 的键不同（reader/screening 不串会话）', () => {
      expect(threadMappingKey('p1', 'paperReader')).not.toBe(threadMappingKey('p1', 'screening'));
    });

    it('set/get/clear 往返', () => {
      setStoredThreadId('p1', 'paperReader', 't-1');
      expect(getStoredThreadId('p1', 'paperReader')).toBe('t-1');
      expect(getStoredThreadId('p1', 'screening')).toBeNull();
      clearStoredThreadId('p1', 'paperReader');
      expect(getStoredThreadId('p1', 'paperReader')).toBeNull();
    });
  });

  describe('probeAgentMode：失败一律降级 ephemeral', () => {
    it('mode=runtime 时返回 runtime', async () => {
      (globalThis.fetch as any).mockResolvedValue({
        ok: true,
        json: async () => ({ mode: 'runtime' }),
      });
      expect(await probeAgentMode()).toBe('runtime');
    });

    it('无 mode 字段 → ephemeral', async () => {
      (globalThis.fetch as any).mockResolvedValue({
        ok: true,
        json: async () => ({ foo: 1 }),
      });
      expect(await probeAgentMode()).toBe('ephemeral');
    });

    it('非 200 → ephemeral', async () => {
      (globalThis.fetch as any).mockResolvedValue({ ok: false, status: 404 });
      expect(await probeAgentMode()).toBe('ephemeral');
    });

    it('网络错 / 非 JSON → ephemeral', async () => {
      (globalThis.fetch as any).mockRejectedValue(new Error('network down'));
      expect(await probeAgentMode()).toBe('ephemeral');
    });
  });

  describe('ensureMainThreadSession', () => {
    it('已有映射直接复用，不发请求', async () => {
      setStoredThreadId('p1', 'paperReader', 't-existing');
      const id = await ensureMainThreadSession('p1', 'paperReader');
      expect(id).toBe('t-existing');
      expect(globalThis.fetch).not.toHaveBeenCalled();
    });

    it('无映射时创建会话并落映射', async () => {
      (globalThis.fetch as any).mockResolvedValue({
        ok: true,
        json: async () => ({ thread_id: 't-new' }),
      });
      const id = await ensureMainThreadSession('p1', 'paperReader', '论文标题');
      expect(id).toBe('t-new');
      expect(getStoredThreadId('p1', 'paperReader')).toBe('t-new');

      const [url, init] = (globalThis.fetch as any).mock.calls[0];
      expect(url).toBe('/api/v1/agent/threads');
      expect(init.method).toBe('POST');
      expect(JSON.parse(init.body)).toEqual({
        agent_type: 'paperReader',
        title: '论文标题',
      });
    });

    it('创建失败向上抛错（调用方决定是否降级）', async () => {
      (globalThis.fetch as any).mockResolvedValue({
        ok: false,
        status: 500,
        json: async () => ({ error: 'boom' }),
      });
      await expect(ensureMainThreadSession('p1', 'paperReader')).rejects.toThrow('boom');
      expect(getStoredThreadId('p1', 'paperReader')).toBeNull();
    });

    it('同键并发共享一次创建请求（不双开会话）', async () => {
      let resolveFetch: (v: any) => void = () => {};
      (globalThis.fetch as any).mockReturnValue(
        new Promise((resolve) => {
          resolveFetch = resolve;
        }),
      );

      const p1 = ensureMainThreadSession('p1', 'paperReader');
      const p2 = ensureMainThreadSession('p1', 'paperReader');
      resolveFetch({ ok: true, json: async () => ({ thread_id: 't-once' }) });

      expect(await p1).toBe('t-once');
      expect(await p2).toBe('t-once');
      expect(globalThis.fetch).toHaveBeenCalledTimes(1);
    });
  });
});
