/**
 * agentThreadsApi — runtime 会话模式（thread ≡ agent session）的 API 客户端
 * ============================================================
 *
 * 后端（tui-http `runtime-host` feature，见 docs/design/web-agent-runtime.md）：
 *   GET    /api/v1/agent                  → { mode: "runtime" | "ephemeral" }
 *   POST   /api/v1/agent/threads          → { thread_id }（create_session）
 *   GET    /api/v1/agent/threads/:id/messages → 服务端转写（快照+WAL）
 *   POST   /api/v1/agent/threads/:id/chat → SSE（契约同 /agent/chat）
 *   PATCH  /api/v1/agent/threads/:id      → rename_session
 *   DELETE /api/v1/agent/threads/:id?agent_type=... → close_session
 *
 * 前端策略（P2）：
 * - 模式探测失败/非 runtime → 一律退回 ephemeral 旧路径（desktop P3 前依赖此降级）
 * - 主对话按 (scope, agent_type) 惰性建 thread，映射存 localStorage
 *   （bib blob 的 conversations 多会话是 jayread 遗产，当前 UI 无切换入口，
 *   scope 即活跃会话；agent_type 并入键避免 reader/screening 串会话）
 * - 旧 bib_meta 时代的对话（UI 已有历史且无映射）继续走 ephemeral，
 *   不做历史搬迁（P4 迁移项）
 *
 * @module ai-chat/api/agentThreadsApi
 */

import { getAuthToken } from '../../../services/client';
import { scopeFor } from '../../../services/chatApi';
import type { AgentType } from '../agentTypes';

export type AgentMode = 'runtime' | 'ephemeral';

/** 会话消息（GET /threads/:id/messages 的条目） */
export interface ThreadMessage {
  role: 'user' | 'assistant';
  text: string;
}

// ============================================================
// 模式探测（模块级缓存）
// ============================================================

let modePromise: Promise<AgentMode> | null = null;

/** 测试注入：固定模式或清除缓存（仅测试用）。 */
export function __setAgentModeForTests(mode: AgentMode | null): void {
  modePromise = mode ? Promise.resolve(mode) : null;
}

/**
 * 探测后端 agent 能力。任何失败（网络错、非 JSON、无 mode 字段）都降级为
 * 'ephemeral' —— 旧端点永远存在，降级是安全方向。
 */
export function probeAgentMode(): Promise<AgentMode> {
  if (!modePromise) {
    modePromise = (async () => {
      try {
        const token = getAuthToken();
        const res = await fetch('/api/v1/agent', {
          headers: token ? { Authorization: `Bearer ${token}` } : {},
        });
        if (!res.ok) return 'ephemeral';
        const data = await res.json();
        return data?.mode === 'runtime' ? 'runtime' : 'ephemeral';
      } catch {
        return 'ephemeral';
      }
    })();
  }
  return modePromise;
}

// ============================================================
// thread ↔ conversation 映射（localStorage）
// ============================================================

const MAPPING_PREFIX = 'agentThreadSession:';

/**
 * 映射键 = scope + agentType。scope 单独不够：paperReader 与 screening 可能
 * 各自对同一篇论文持有会话，而它们是不同的 resident agent（transcript 存在
 * 各自的 agent 存储里），同一个 scope 复用 thread 会把消息串进别人的会话。
 */
export function threadMappingKey(
  paperId: string | number | null | undefined,
  agentType: AgentType,
): string {
  return `${MAPPING_PREFIX}${scopeFor(paperId == null ? undefined : String(paperId))}|${agentType}`;
}

export function getStoredThreadId(
  paperId: string | number | null | undefined,
  agentType: AgentType,
): string | null {
  try {
    return localStorage.getItem(threadMappingKey(paperId, agentType));
  } catch {
    return null; // 隐私模式等 localStorage 不可用 — 退化为每次重建（会话仍在服务端，仅丢映射）
  }
}

export function setStoredThreadId(
  paperId: string | number | null | undefined,
  agentType: AgentType,
  threadId: string,
): void {
  try {
    localStorage.setItem(threadMappingKey(paperId, agentType), threadId);
  } catch {
    // ignore
  }
}

export function clearStoredThreadId(
  paperId: string | number | null | undefined,
  agentType: AgentType,
): void {
  try {
    localStorage.removeItem(threadMappingKey(paperId, agentType));
  } catch {
    // ignore
  }
}

// ============================================================
// /threads API
// ============================================================

function authHeaders(): Record<string, string> {
  const token = getAuthToken();
  return token ? { Authorization: `Bearer ${token}` } : {};
}

async function errorMessageFrom(res: Response, fallback: string): Promise<string> {
  try {
    const data = await res.json();
    return data?.error || data?.message || fallback;
  } catch {
    return fallback;
  }
}

export interface CreateThreadParams {
  agent_type: AgentType;
  title?: string;
  /** 分叉父会话（追问线程的未来路径，P4） */
  fork_from?: string;
}

/** 创建 thread（后端 create_session）。失败抛 Error（带服务端信息）。 */
export async function createThread(params: CreateThreadParams): Promise<string> {
  const res = await fetch('/api/v1/agent/threads', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...authHeaders() },
    body: JSON.stringify(params),
  });
  if (!res.ok) {
    throw new Error(await errorMessageFrom(res, `创建会话失败 (${res.status})`));
  }
  const data = await res.json();
  return data?.thread_id;
}

/** 读服务端转写；404/出错返回 null（调用方按空对话处理）。 */
export async function fetchThreadMessages(threadId: string): Promise<ThreadMessage[] | null> {
  try {
    const res = await fetch(`/api/v1/agent/threads/${encodeURIComponent(threadId)}/messages`, {
      headers: authHeaders(),
    });
    if (!res.ok) return null;
    const data = await res.json();
    return Array.isArray(data?.messages) ? data.messages : null;
  } catch {
    return null;
  }
}

/** 删除 thread（close_session）。失败静默（清空对话是尽力而为的清理）。 */
export async function deleteThread(threadId: string, agentType: AgentType): Promise<void> {
  try {
    await fetch(
      `/api/v1/agent/threads/${encodeURIComponent(threadId)}?agent_type=${encodeURIComponent(agentType)}`,
      { method: 'DELETE', headers: authHeaders() },
    );
  } catch {
    // ignore
  }
}

/**
 * 取当前对话的 thread_id；没有则创建并落映射。
 * 并发安全：同 scope 的并发请求共享同一个 in-flight Promise，避免双开会话。
 */
const ensuring = new Map<string, Promise<string>>();

export async function ensureMainThreadSession(
  paperId: string | number | null | undefined,
  agentType: AgentType,
  title?: string,
): Promise<string> {
  const key = threadMappingKey(paperId, agentType);
  const stored = getStoredThreadId(paperId, agentType);
  if (stored) return stored;

  const inflight = ensuring.get(key);
  if (inflight) return inflight;

  const p = createThread({ agent_type: agentType, title })
    .then((threadId) => {
      setStoredThreadId(paperId, agentType, threadId);
      return threadId;
    })
    .finally(() => ensuring.delete(key));
  ensuring.set(key, p);
  return p;
}

/** thread chat 端点（SSE，契约与 /api/v1/agent/chat 完全一致）。 */
export function threadChatUrl(threadId: string): string {
  return `/api/v1/agent/threads/${encodeURIComponent(threadId)}/chat`;
}
