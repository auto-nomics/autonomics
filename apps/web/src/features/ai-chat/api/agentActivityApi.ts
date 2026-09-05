/**
 * agentActivityApi — P5a 只读观测 API 客户端（docs/design/web-agent-delegation.md §4）
 * ============================================================
 *
 * 后端（tui-http `runtime-host` feature，挂在 /api/v1/agent 下，bearer 保护）：
 *   GET /api/v1/agent/agents               → 活跃 + 持久化 agent 图（按 path 去重，live 优先）
 *   GET /api/v1/agent/delegations?status=&target=&limit= → delegation 台账（newest first）
 *   GET /api/v1/agent/agents/history?agent=&limit= → agent 转写（{role,text} 同 threads messages）
 *   GET /api/v1/agent/agents/events        → host 实时事件 SSE（registered / unregistered /
 *                                            status / output + keepalive，P5c-5）
 *
 * 设计约束：
 * - 只在 runtime 模式下有这些端点；ephemeral 宿主 404，调用方（活动抽屉）
 *   依赖 probeAgentMode 的降级不渲染入口，模块本身失败一律返回 null /
 *   空数组，不抛错（抽屉显示错误态 + 重试，不打断聊天）。
 * - `agent` 是 query 参数而非路径段——完整 agent 路径（/root/web/homepage）
 *   含斜杠，走 query 免编码纠缠。
 *
 * @module ai-chat/api/agentActivityApi
 */

import { getAuthToken } from '../../../services/client';

/** GET /agents 的单行：live 行来自 host 注册表或常驻 web agent，非 live 行来自持久化 agent 图 */
export interface AgentRow {
  name: string;
  path: string;
  agent_id: string | null;
  status: string; // idle | running | awaiting_tool | completed | failed | unknown（非 live 行可能是持久化的历史状态）
  live: boolean;
  last_event?: string | null;
  summary?: string;
  tags?: string[];
  tools?: string[];
  parent_path?: string | null;
  profile_path?: string | null;
}

/** GET /delegations 的单行（后端 DelegationSnapshot 直出，snake_case） */
export interface DelegationRow {
  delegation_id: string;
  caller_path: string | null;
  target_path: string;
  task: string;
  status: 'pending' | 'running' | 'completed' | 'interrupted' | 'failed';
  turn_id: string | null;
  session_id: string | null;
  response: string | null;
  created_at: number;
  updated_at: number;
}

/** GET /agents/history 的响应 */
export interface AgentHistory {
  agent_path: string;
  agent_id: string | null;
  session_id: string | null;
  messages: Array<{ role: 'user' | 'assistant'; text: string }>;
}

function authHeaders(): Record<string, string> {
  const token = getAuthToken();
  return token ? { Authorization: `Bearer ${token}` } : {};
}

/** 拉取 agent 全景（活跃 + 历史图）。失败返回 null（抽屉显示错误态）。 */
export async function fetchAgents(): Promise<AgentRow[] | null> {
  try {
    const res = await fetch('/api/v1/agent/agents', { headers: authHeaders() });
    if (!res.ok) return null;
    const data = await res.json();
    return Array.isArray(data?.agents) ? data.agents : null;
  } catch {
    return null;
  }
}

export interface DelegationsParams {
  /** 终态过滤（五态之一，后端校验） */
  status?: string;
  /** 目标 agent 过滤（全路径或短名） */
  target?: string;
  limit?: number;
}

/** 拉取 delegation 台账（newest first）。失败返回 null。 */
export async function fetchDelegations(params: DelegationsParams = {}): Promise<DelegationRow[] | null> {
  try {
    const query = new URLSearchParams();
    if (params.status) query.set('status', params.status);
    if (params.target) query.set('target', params.target);
    if (params.limit != null) query.set('limit', String(params.limit));
    const qs = query.toString();
    const res = await fetch(`/api/v1/agent/delegations${qs ? `?${qs}` : ''}`, {
      headers: authHeaders(),
    });
    if (!res.ok) return null;
    const data = await res.json();
    return Array.isArray(data?.delegations) ? data.delegations : null;
  } catch {
    return null;
  }
}

/**
 * 拉 agent 近期转写。`agent` 接受完整路径（/root/web/homepage）、短名
 * （homepage）或 agent_type 键（paperReader）；未知 agent 返回空历史而非错误。
 */
export async function fetchAgentHistory(agent: string, limit = 20): Promise<AgentHistory | null> {
  try {
    const query = new URLSearchParams({ agent });
    if (limit != null) query.set('limit', String(limit));
    const res = await fetch(`/api/v1/agent/agents/history?${query.toString()}`, {
      headers: authHeaders(),
    });
    if (!res.ok) return null;
    const data = await res.json();
    if (!data || !Array.isArray(data.messages)) return null;
    return data as AgentHistory;
  } catch {
    return null;
  }
}

// ──────────────────────────────────────────────────────────────────────
// P5c-6: host 实时事件订阅（GET /agents/events SSE）
// ──────────────────────────────────────────────────────────────────────

/** SSE 帧（wire 事件名 → type；`sync` 为断线重连后的本地合成标记，非后端帧） */
export type AgentEventFrame =
  | { type: 'registered'; path: string; name: string; status: string }
  | { type: 'unregistered'; path: string }
  | { type: 'status'; path: string; status: string; last_event: string | null }
  | { type: 'output'; path: string; text: string }
  | { type: 'sync' };

/** 断线重连退避（抽屉长开场景下 SSE 被代理掐断后的自愈间隔） */
const SSE_RECONNECT_MS = 3000;

/** 解析一个 `\n\n` 分隔的 SSE 块（event:/data: 行）；keepalive 注释块、无 data
 *  的块、未知事件与坏 JSON 一律返回 null（与后端契约：data 恒为单行 JSON）。 */
function parseSseBlock(block: string): AgentEventFrame | null {
  let event = '';
  let data = '';
  for (const line of block.split('\n')) {
    if (line.startsWith('event:')) event = line.slice(6).trim();
    else if (line.startsWith('data:')) data += line.slice(5).trim();
  }
  if (!event || !data) return null;
  let payload: Record<string, unknown>;
  try {
    payload = JSON.parse(data) as Record<string, unknown>;
  } catch {
    return null;
  }
  const path = typeof payload.path === 'string' ? payload.path : '';
  switch (event) {
    case 'registered':
      return {
        type: 'registered',
        path,
        name: typeof payload.name === 'string' ? payload.name : path,
        status: typeof payload.status === 'string' ? payload.status : 'unknown',
      };
    case 'unregistered':
      return { type: 'unregistered', path };
    case 'status':
      return {
        type: 'status',
        path,
        status: typeof payload.status === 'string' ? payload.status : 'unknown',
        last_event: typeof payload.last_event === 'string' ? payload.last_event : null,
      };
    case 'output':
      return {
        type: 'output',
        path,
        text: typeof payload.text === 'string' ? payload.text : '',
      };
    default:
      return null;
  }
}

/**
 * 订阅 host 实时事件流（/agents/events SSE）。
 *
 * 不用 EventSource——它带不了 Authorization 头，走 fetch + ReadableStream 手解
 * SSE 线格式（event:/data: 块，`\n\n` 分隔；keepalive 注释块自然被忽略）。
 * 流断开（网络错 / 非 ok / 服务端关流）后每 3s 重连；重连成功先回调
 * `{type:'sync'}` 提示调用方重拉快照（SSE 是增量喂给，断线期间的帧丢了）。
 * 返回 unsubscribe：中断请求 + 停止重连。
 */
export function subscribeAgentEvents(onEvent: (frame: AgentEventFrame) => void): () => void {
  let closed = false;
  const controller = new AbortController();

  const run = async () => {
    let everConnected = false;
    while (!closed) {
      try {
        const res = await fetch('/api/v1/agent/agents/events', {
          headers: { Accept: 'text/event-stream', ...authHeaders() },
          signal: controller.signal,
        });
        if (!res.ok || !res.body) throw new Error(`sse status ${res.status}`);
        if (everConnected) onEvent({ type: 'sync' });
        everConnected = true;

        const reader = res.body.getReader();
        const decoder = new TextDecoder();
        let buffer = '';
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });
          let sep = buffer.indexOf('\n\n');
          while (sep !== -1) {
            const frame = parseSseBlock(buffer.slice(0, sep));
            buffer = buffer.slice(sep + 2);
            if (frame && !closed) onEvent(frame);
            sep = buffer.indexOf('\n\n');
          }
        }
      } catch {
        // 中断（closed）或网络错：落到下面的退避重连
      }
      if (closed) break;
      await new Promise<void>((resolve) => setTimeout(resolve, SSE_RECONNECT_MS));
    }
  };
  void run();

  return () => {
    closed = true;
    controller.abort();
  };
}
