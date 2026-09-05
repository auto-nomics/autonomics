/**
 * agentThreadsApi 单元测试
 *
 * 覆盖的契约：
 * 1. 映射键归一：真实 ID / safeId / undefined 三种输入收敛到同一键，
 *    agentType 参与键（reader 与 screening 不串会话）
 * 2. 模式探测降级：任何失败（非 JSON / 无 mode / 网络错）都落 'ephemeral'，
 *    desktop 与旧后端因此永远走旧端点
 * 3. ensureMainThreadSession 并发去重：同键并发只发一次创建请求；
 *    P4 起 legacyMessages 触发 import 路径（含 404 降级 create）
 * 4. P4 inline 线程映射：thread: 前缀键隔离 + 按前缀批量清理
 * 5. P4 toImportMessages：UI 消息归一为 import 载荷
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  __setAgentModeForTests,
  probeAgentMode,
  threadMappingKey,
  getStoredThreadId,
  setStoredThreadId,
  clearStoredThreadId,
  inlineThreadMappingKey,
  getInlineThreadSessionId,
  setInlineThreadSessionId,
  clearInlineThreadSessionId,
  clearInlineThreadSessionIds,
  importThread,
  toImportMessages,
  ensureMainThreadSession,
} from '../agentThreadsApi';

// happy-dom 暴露的 localStorage 缺方法（client.test.ts 同款问题）：内存 shim。
// length/key(i) 是 clearInlineThreadSessionIds 前缀扫描的依赖，一并补上
const storageStore = new Map<string, string>();
const localStorageShim = {
  getItem: (k: string) => (storageStore.has(k) ? (storageStore.get(k) as string) : null),
  setItem: (k: string, v: string) => { storageStore.set(k, String(v)); },
  removeItem: (k: string) => { storageStore.delete(k); },
  clear: () => storageStore.clear(),
  get length() { return storageStore.size; },
  key: (i: number) => Array.from(storageStore.keys())[i] ?? null,
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

    describe('P4：legacyMessages 走 import 路径', () => {
      const legacy = [
        { role: 'user' as const, content: '旧问题' },
        { role: 'assistant' as const, content: '旧回答' },
      ];

      it('有历史无映射 → /threads/import（带 title），成功后落映射', async () => {
        (globalThis.fetch as any).mockResolvedValue({
          ok: true,
          json: async () => ({ thread_id: 't-imported' }),
        });
        const id = await ensureMainThreadSession('p1', 'paperReader', '论文标题', legacy);
        expect(id).toBe('t-imported');
        expect(getStoredThreadId('p1', 'paperReader')).toBe('t-imported');

        const [url, init] = (globalThis.fetch as any).mock.calls[0];
        expect(url).toBe('/api/v1/agent/threads/import');
        expect(JSON.parse(init.body)).toEqual({
          agent_type: 'paperReader',
          title: '论文标题',
          messages: legacy,
        });
      });

      it('import 404（旧 runtime 后端无路由）→ 降级 create 空会话', async () => {
        (globalThis.fetch as any)
          .mockResolvedValueOnce({ ok: false, status: 404, json: async () => ({ error: 'no route' }) })
          .mockResolvedValueOnce({ ok: true, json: async () => ({ thread_id: 't-fresh' }) });

        const id = await ensureMainThreadSession('p1', 'paperReader', undefined, legacy);
        expect(id).toBe('t-fresh');
        expect(getStoredThreadId('p1', 'paperReader')).toBe('t-fresh');
        const [secondUrl] = (globalThis.fetch as any).mock.calls[1];
        expect(secondUrl).toBe('/api/v1/agent/threads');
      });

      it('import 非 404 失败 → 向上抛（调用方本轮报错）', async () => {
        (globalThis.fetch as any).mockResolvedValue({
          ok: false,
          status: 500,
          json: async () => ({ error: 'storage down' }),
        });
        await expect(ensureMainThreadSession('p1', 'paperReader', undefined, legacy)).rejects.toThrow(
          'storage down',
        );
        expect(getStoredThreadId('p1', 'paperReader')).toBeNull();
      });

      it('legacyMessages 为空数组 → 直接 create（不打 import 端点）', async () => {
        (globalThis.fetch as any).mockResolvedValue({
          ok: true,
          json: async () => ({ thread_id: 't-new' }),
        });
        await ensureMainThreadSession('p1', 'paperReader', undefined, []);
        const [url] = (globalThis.fetch as any).mock.calls[0];
        expect(url).toBe('/api/v1/agent/threads');
      });
    });
  });

  describe('P4：inline 线程映射', () => {
    it('键 = 主键 + thread:{uiThreadId}（子线程与一级线程各自独立）', () => {
      const main = threadMappingKey('p1', 'paperReader');
      expect(inlineThreadMappingKey('p1', 'paperReader', 't1')).toBe(`${main}|thread:t1`);
      expect(inlineThreadMappingKey('p1', 'paperReader', 't1')).not.toBe(
        inlineThreadMappingKey('p1', 'paperReader', 'sub1'),
      );
    });

    it('get/set/clear 往返，不影响主对话映射', () => {
      setStoredThreadId('p1', 'paperReader', 't-main');
      setInlineThreadSessionId('p1', 'paperReader', 't1', 's-1');
      expect(getInlineThreadSessionId('p1', 'paperReader', 't1')).toBe('s-1');
      expect(getStoredThreadId('p1', 'paperReader')).toBe('t-main');
      clearInlineThreadSessionId('p1', 'paperReader', 't1');
      expect(getInlineThreadSessionId('p1', 'paperReader', 't1')).toBeNull();
      expect(getStoredThreadId('p1', 'paperReader')).toBe('t-main');
    });

    it('clearInlineThreadSessionIds：按前缀清空并返回 sessionId，别的 scope 不受影响', () => {
      setInlineThreadSessionId('p1', 'paperReader', 't1', 's-1');
      setInlineThreadSessionId('p1', 'paperReader', 't2', 's-2');
      setInlineThreadSessionId('p2', 'paperReader', 't9', 's-other-paper');
      setInlineThreadSessionId('p1', 'screening', 't1', 's-other-type');
      setStoredThreadId('p1', 'paperReader', 't-main');

      const removed = clearInlineThreadSessionIds('p1', 'paperReader');
      expect(removed.sort()).toEqual(['s-1', 's-2']);
      expect(getInlineThreadSessionId('p1', 'paperReader', 't1')).toBeNull();
      expect(getInlineThreadSessionId('p1', 'paperReader', 't2')).toBeNull();
      // 不同 scope / agentType / 主对话映射原样保留
      expect(getInlineThreadSessionId('p2', 'paperReader', 't9')).toBe('s-other-paper');
      expect(getInlineThreadSessionId('p1', 'screening', 't1')).toBe('s-other-type');
      expect(getStoredThreadId('p1', 'paperReader')).toBe('t-main');
    });
  });

  describe('P4：importThread / toImportMessages', () => {
    it('importThread 成功返回 thread_id，失败抛带 status 的 Error', async () => {
      (globalThis.fetch as any).mockResolvedValueOnce({
        ok: true,
        json: async () => ({ thread_id: 't-ok' }),
      });
      expect(
        await importThread({
          agent_type: 'homepage',
          messages: [{ role: 'user', content: 'hi' }],
        }),
      ).toBe('t-ok');

      (globalThis.fetch as any).mockResolvedValueOnce({
        ok: false,
        status: 400,
        json: async () => ({ error: 'messages must contain at least one user/assistant turn' }),
      });
      const err = await importThread({
        agent_type: 'homepage',
        messages: [],
      }).catch((e) => e);
      expect(err).toBeInstanceOf(Error);
      expect(err.message).toContain('user/assistant');
      expect(err.status).toBe(400);
    });

    it('toImportMessages：留 user/assistant，拼数组 content，滤 system/空文本/异型', () => {
      expect(
        toImportMessages([
          { role: 'system', content: 'persona' },
          { role: 'user', content: '纯文本问题' },
          { role: 'assistant', content: [{ type: 'text', text: '分段一' }, { type: 'text', text: '分段二' }] },
          { role: 'user', content: '   ' },            // 空白文本丢弃
          { role: 'user', content: [{ type: 'image_url' }] }, // 无 text 块 → 空串丢弃
          { role: 'assistant', content: 42 },          // 异型丢弃
        ]),
      ).toEqual([
        { role: 'user', content: '纯文本问题' },
        { role: 'assistant', content: '分段一\n分段二' },
      ]);
    });
  });
});
