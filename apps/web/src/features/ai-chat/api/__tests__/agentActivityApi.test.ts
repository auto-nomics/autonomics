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
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  fetchAgents,
  fetchDelegations,
  fetchAgentHistory,
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
});
