/**
 * agentActivityApi 单元测试（P5a 只读观测）
 *
 * 覆盖的契约：
 * 1. fetchAgents：路径 /api/v1/agent/agents + bearer 头 + agents 数组解包；
 *    非 ok / 网络错 / 形状不对 → null（抽屉错误态，不抛错）
 * 2. fetchDelegations：status/target/limit 进 query（缺省不发空参数）；
 *    delegations 数组解包；失败 → null
 * 3. fetchAgentHistory：agent 走 query 参数（完整路径含斜杠不进路径段）、
 *    limit 默认 20；messages 形状校验；失败 → null
 * 4. subscribeAgentEvents（P5c-6）：fetch 流式解析 SSE（event:/data: 块、
 *    跨 chunk 重组、keepalive/未知事件忽略）、bearer + Accept 头、
 *    unsubscribe 中止请求并停投递、断流后 3s 重连且重连成功先发 sync
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  fetchAgents,
  fetchDelegations,
  fetchAgentHistory,
  subscribeAgentEvents,
  type AgentEventFrame,
} from '../agentActivityApi';
import { getAuthToken } from '../../../../services/client';

vi.mock('../../../../services/client', () => ({
  getAuthToken: vi.fn(() => 'test-token'),
}));

const okJson = (body: unknown) =>
  Promise.resolve({ ok: true, json: () => Promise.resolve(body) } as Response);

describe('agentActivityApi', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn());
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  describe('fetchAgents', () => {
    it('GET /api/v1/agent/agents，带 bearer，解包 agents 数组', async () => {
      (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
        okJson({
          agents: [
            { name: 'homepage', path: '/root/web/homepage', status: 'idle', live: true, agent_id: 'u1' },
            { name: 'researcher', path: '/root/researcher', status: 'completed', live: false, agent_id: 'u2' },
          ],
        }),
      );
      const agents = await fetchAgents();
      const [call] = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls;
      expect(call[0]).toBe('/api/v1/agent/agents');
      expect(call[1].headers.Authorization).toBe('Bearer test-token');
      expect(agents).toHaveLength(2);
      expect(agents?.[0]).toMatchObject({ path: '/root/web/homepage', live: true });
      expect(agents?.[1]).toMatchObject({ live: false, status: 'completed' });
    });

    it('非 ok / 网络错 / agents 非数组 → null', async () => {
      (globalThis.fetch as ReturnType<typeof vi.fn>)
        .mockResolvedValueOnce({ ok: false, status: 404 } as unknown as Response)
        .mockRejectedValueOnce(new Error('network'))
        .mockResolvedValueOnce(okJson({ nope: true }));
      expect(await fetchAgents()).toBeNull();
      expect(await fetchAgents()).toBeNull();
      expect(await fetchAgents()).toBeNull();
    });

    it('无 token 时不带 Authorization 头', async () => {
      (getAuthToken as unknown as ReturnType<typeof vi.fn>).mockReturnValueOnce(null);
      (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValueOnce(okJson({ agents: [] }));
      await fetchAgents();
      const [call] = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls;
      expect(call[1].headers.Authorization).toBeUndefined();
    });
  });

  describe('fetchDelegations', () => {
    it('status/target/limit 进 query，缺省参数不发', async () => {
      (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
        okJson({ delegations: [] }),
      );
      await fetchDelegations({ status: 'running', target: '/root/web/homepage', limit: 10 });
      await fetchDelegations();
      const calls = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls;
      expect(calls[0][0]).toBe(
        '/api/v1/agent/delegations?status=running&target=%2Froot%2Fweb%2Fhomepage&limit=10',
      );
      expect(calls[1][0]).toBe('/api/v1/agent/delegations');
    });

    it('解包 delegations；非 ok → null', async () => {
      (globalThis.fetch as ReturnType<typeof vi.fn>)
        .mockResolvedValueOnce(
          okJson({
            delegations: [
              {
                delegation_id: 'd1',
                caller_path: null,
                target_path: '/root/web/homepage',
                task: '读论文',
                status: 'completed',
                turn_id: null,
                session_id: null,
                response: 'ok',
                created_at: 1,
                updated_at: 2,
              },
            ],
          }),
        )
        .mockResolvedValueOnce({ ok: false } as unknown as Response);
      const rows = await fetchDelegations({ status: 'completed' });
      expect(rows).toHaveLength(1);
      expect(rows?.[0]).toMatchObject({ delegation_id: 'd1', status: 'completed' });
      expect(await fetchDelegations()).toBeNull();
    });
  });

  describe('fetchAgentHistory', () => {
    it('agent 与 limit 走 query 参数（完整路径含斜杠不进路径段）', async () => {
      (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
        okJson({
          agent_path: '/root/web/homepage',
          agent_id: 'u1',
          session_id: 's1',
          messages: [{ role: 'user', text: 'hi' }],
        }),
      );
      const history = await fetchAgentHistory('/root/web/homepage');
      const [call] = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls;
      expect(call[0]).toBe(
        '/api/v1/agent/agents/history?agent=%2Froot%2Fweb%2Fhomepage&limit=20',
      );
      expect(history?.messages).toEqual([{ role: 'user', text: 'hi' }]);
    });

    it('messages 缺失 / 非 ok → null；网络错 → null', async () => {
      (globalThis.fetch as ReturnType<typeof vi.fn>)
        .mockResolvedValueOnce(okJson({ agent_path: 'x' }))
        .mockResolvedValueOnce({ ok: false } as unknown as Response)
        .mockRejectedValueOnce(new Error('network'));
      expect(await fetchAgentHistory('nobody')).toBeNull();
      expect(await fetchAgentHistory('nobody')).toBeNull();
      expect(await fetchAgentHistory('nobody')).toBeNull();
    });
  });

  describe('subscribeAgentEvents', () => {
    /** 一个后端同款 SSE 帧（event:/data: 块，`\n\n` 结尾） */
    const frame = (event: string, data: unknown) =>
      `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;

    /** 立即灌完 chunks 并关流的 Response 桩 */
    const sseResponse = (chunks: string[]): Response => {
      const encoder = new TextEncoder();
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
          controller.close();
        },
      });
      return { ok: true, status: 200, body } as unknown as Response;
    };

    /** 排空微任务链（fetch resolve → read → read(done) → 排 3s 重连定时器） */
    const drainMicrotasks = async (times = 20) => {
      for (let i = 0; i < times; i += 1) await Promise.resolve();
    };

    it('流式解析：跨 chunk 重组半帧、keepalive 注释与未知事件忽略', async () => {
      const events: AgentEventFrame[] = [];
      (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
        sseResponse([
          frame('registered', { path: '/root/r', name: 'r', status: 'idle' }),
          'event: out', // 半帧：跨 chunk 重组
          'put\ndata: {"path":"/root/r","text":"第1轮"}\n\n',
          ': keepalive\n\n', // axum KeepAlive 注释块 → 无 data，忽略
          'event: ping\ndata: {}\n\n', // 未知事件 → 忽略
          frame('status', { path: '/root/r', status: 'running', last_event: 'x' }),
          frame('unregistered', { path: '/root/r' }),
        ]),
      );
      const unsubscribe = subscribeAgentEvents((f) => events.push(f));
      await vi.waitFor(() => expect(events.length).toBe(4));
      expect(events[0]).toEqual({ type: 'registered', path: '/root/r', name: 'r', status: 'idle' });
      expect(events[1]).toEqual({ type: 'output', path: '/root/r', text: '第1轮' });
      expect(events[2]).toEqual({ type: 'status', path: '/root/r', status: 'running', last_event: 'x' });
      expect(events[3]).toEqual({ type: 'unregistered', path: '/root/r' });
      unsubscribe();
    });

    it('请求形状：/agents/events + bearer + Accept + signal；unsubscribe 中止并停投递', async () => {
      const events: AgentEventFrame[] = [];
      let streamController: ReadableStreamDefaultController<Uint8Array> | undefined;
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          streamController = controller;
          controller.enqueue(new TextEncoder().encode(frame('registered', { path: '/root/r', name: 'r', status: 'idle' })));
          // 不关流：模拟长连接
        },
      });
      (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
        ok: true,
        status: 200,
        body,
      } as unknown as Response);
      const unsubscribe = subscribeAgentEvents((f) => events.push(f));
      await vi.waitFor(() => expect(events).toHaveLength(1));

      const [call] = (globalThis.fetch as ReturnType<typeof vi.fn>).mock.calls;
      expect(call[0]).toBe('/api/v1/agent/agents/events');
      expect(call[1].headers.Authorization).toBe('Bearer test-token');
      expect(call[1].headers.Accept).toBe('text/event-stream');
      expect(call[1].signal).toBeInstanceOf(AbortSignal);

      unsubscribe();
      expect(call[1].signal.aborted).toBe(true);
      // 退订后到达的帧不投递（closed 守卫）
      streamController!.enqueue(new TextEncoder().encode(frame('output', { path: '/root/r', text: 'late' })));
      streamController!.close();
      await drainMicrotasks();
      expect(events).toHaveLength(1);
    });

    it('断流后 3s 重连；非 ok 同样退避；重连成功先发 sync', async () => {
      vi.useFakeTimers();
      try {
        const events: AgentEventFrame[] = [];
        (globalThis.fetch as ReturnType<typeof vi.fn>)
          .mockResolvedValueOnce(sseResponse([frame('output', { path: '/r', text: 'a' })]))
          .mockResolvedValueOnce({ ok: false, status: 502 } as unknown as Response)
          .mockResolvedValueOnce(sseResponse([frame('output', { path: '/r', text: 'b' })]));
        const unsubscribe = subscribeAgentEvents((f) => events.push(f));
        await drainMicrotasks();
        expect(events).toEqual([{ type: 'output', path: '/r', text: 'a' }]);

        await vi.advanceTimersByTimeAsync(3000); // 第 1 次重连：502 → 再退避
        expect((globalThis.fetch as ReturnType<typeof vi.fn>)).toHaveBeenCalledTimes(2);
        expect(events).toHaveLength(1); // 失败连接不产生帧

        await vi.advanceTimersByTimeAsync(3000); // 第 2 次重连：成功 → sync 补基线
        await drainMicrotasks();
        expect((globalThis.fetch as ReturnType<typeof vi.fn>)).toHaveBeenCalledTimes(3);
        expect(events.slice(1)).toEqual([{ type: 'sync' }, { type: 'output', path: '/r', text: 'b' }]);
        unsubscribe();
      } finally {
        vi.useRealTimers();
      }
    });
  });
});
