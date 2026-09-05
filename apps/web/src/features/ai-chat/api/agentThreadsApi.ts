/**
 * agentThreadsApi — runtime 会话模式（thread ≡ agent session）的 API 客户端
 * ============================================================
 *
 * 后端（tui-http `runtime-host` feature，见 docs/design/web-agent-runtime.md）：
 *   GET    /api/v1/agent                  → { mode: "runtime" | "ephemeral" }
 *   POST   /api/v1/agent/threads          → { thread_id }（create_session）
 *   POST   /api/v1/agent/threads/import   → { thread_id }（存量转写导入，P4）
 *   GET    /api/v1/agent/threads/:id/messages → 服务端转写（快照+WAL）
 *   POST   /api/v1/agent/threads/:id/chat → SSE（契约同 /agent/chat）
 *   PATCH  /api/v1/agent/threads/:id      → rename_session
 *   DELETE /api/v1/agent/threads/:id?agent_type=... → close_session
 *
 * 前端策略（P4）：
 * - 模式探测失败/非 runtime → 一律退回 ephemeral 旧路径（desktop P3 前依赖此降级）
 * - 主对话按 (scope, agent_type) 惰性建 thread，映射存 localStorage
 *   （bib blob 的 conversations 多会话是 jayread 遗产，当前 UI 无切换入口，
 *   scope 即活跃会话；agent_type 并入键避免 reader/screening 串会话）
 * - 旧 bib_meta 时代的对话（UI 已有历史且无映射）在 runtime 模式下经
 *   /threads/import 迁入服务端 session（P4；bib blob 仍保留作 UI 结构锚点）
 * - 追问（inline）线程 = 独立 session，映射键在主键上追加 `thread:{uiThreadId}`
 *   （uiThreadId = subThreadId ?? threadId，子线程与一级线程各自独立）
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

/**
 * 追问（inline）线程的映射键 = 主键 + `|thread:{uiThreadId}`。
 * uiThreadId = subThreadId ?? threadId：子线程与一级线程是不同的对话流，
 * 各自独立 session；`thread:` 前缀把它们与主对话键区分开。
 */
export function inlineThreadMappingKey(
  paperId: string | number | null | undefined,
  agentType: AgentType,
  uiThreadId: string,
): string {
  return `${threadMappingKey(paperId, agentType)}|thread:${uiThreadId}`;
}

export function getInlineThreadSessionId(
  paperId: string | number | null | undefined,
  agentType: AgentType,
  uiThreadId: string,
): string | null {
  try {
    return localStorage.getItem(inlineThreadMappingKey(paperId, agentType, uiThreadId));
  } catch {
    return null;
  }
}

export function setInlineThreadSessionId(
  paperId: string | number | null | undefined,
  agentType: AgentType,
  uiThreadId: string,
  sessionId: string,
): void {
  try {
    localStorage.setItem(inlineThreadMappingKey(paperId, agentType, uiThreadId), sessionId);
  } catch {
    // ignore
  }
}

export function clearInlineThreadSessionId(
  paperId: string | number | null | undefined,
  agentType: AgentType,
  uiThreadId: string,
): void {
  try {
    localStorage.removeItem(inlineThreadMappingKey(paperId, agentType, uiThreadId));
  } catch {
    // ignore
  }
}

/**
 * 清掉该 scope 的全部 inline 线程映射（/clear 时消息列表连线程一起消失，
 * 逐个 uiThreadId 清不可行——按键前缀扫描）。返回被清掉的 sessionId 列表
 * （调用方可顺手 deleteThread 清服务端 session）。
 */
export function clearInlineThreadSessionIds(
  paperId: string | number | null | undefined,
  agentType: AgentType,
): string[] {
  const prefix = `${threadMappingKey(paperId, agentType)}|thread:`;
  const removed: string[] = [];
  try {
    const keys: string[] = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key && key.startsWith(prefix)) keys.push(key);
    }
    for (const key of keys) {
      const sessionId = localStorage.getItem(key);
      if (sessionId) removed.push(sessionId);
      localStorage.removeItem(key);
    }
  } catch {
    // ignore（localStorage 不可用 = 本来就没有映射）
  }
  return removed;
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

/** 创建 thread（后端 create_session）。失败抛带 `status` 的 Error。 */
export async function createThread(params: CreateThreadParams): Promise<string> {
  const res = await fetch('/api/v1/agent/threads', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...authHeaders() },
    body: JSON.stringify(params),
  });
  if (!res.ok) {
    throw Object.assign(new Error(await errorMessageFrom(res, `创建会话失败 (${res.status})`)), {
      status: res.status,
    });
  }
  const data = await res.json();
  return data?.thread_id;
}

// ============================================================
// 存量转写导入（P4 迁移）
// ============================================================

/** import 端点的单条消息（后端仅接受 user/assistant 文本轮次） */
export interface ImportMessage {
  role: 'user' | 'assistant';
  content: string;
}

export interface ImportThreadParams {
  agent_type: AgentType;
  title?: string;
  messages: ImportMessage[];
}

/**
 * 把 UI 消息归一为 import 载荷：只留 user/assistant，content 取文本
 * （数组形态拼接 text 块），无文本的轮次丢弃。主对话 store 消息与
 * prepareThreadContext 的线程历史（字段更宽）都能传入。
 */
export function toImportMessages(
  messages: Array<{ role?: string; content?: unknown }>,
): ImportMessage[] {
  const out: ImportMessage[] = [];
  for (const m of messages) {
    if (!m || (m.role !== 'user' && m.role !== 'assistant')) continue;
    let text: string;
    if (typeof m.content === 'string') {
      text = m.content;
    } else if (Array.isArray(m.content)) {
      text = m.content
        .map((block) =>
          block && typeof block === 'object' && typeof (block as { text?: unknown }).text === 'string'
            ? (block as { text: string }).text
            : '',
        )
        .filter(Boolean)
        .join('\n');
    } else {
      continue;
    }
    if (!text.trim()) continue;
    out.push({ role: m.role, content: text });
  }
  return out;
}

/** 导入存量转写（后端先落库再让活 agent 收养）。失败抛带 `status` 的 Error。 */
export async function importThread(params: ImportThreadParams): Promise<string> {
  const res = await fetch('/api/v1/agent/threads/import', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...authHeaders() },
    body: JSON.stringify(params),
  });
  if (!res.ok) {
    throw Object.assign(new Error(await errorMessageFrom(res, `导入会话失败 (${res.status})`)), {
      status: res.status,
    });
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
 * 取当前对话的 thread_id；没有则按需创建/导入并落映射。
 * - 无 legacyMessages（或为空）→ createThread（空会话）
 * - 有 legacyMessages → importThread（把 bib_meta 时代的历史迁入服务端；
 *   404 = 旧 runtime 后端无 import 路由 → 降级空会话，旧历史仍由 bib blob
 *   渲染，只是服务端上下文从本轮起算）
 * 并发安全：同 scope 的并发请求共享同一个 in-flight Promise，避免
 * useChatInit 预导入与首条发送双开会话（也会双写历史）。
 */
const ensuring = new Map<string, Promise<string>>();

export async function ensureMainThreadSession(
  paperId: string | number | null | undefined,
  agentType: AgentType,
  title?: string,
  legacyMessages?: ImportMessage[],
): Promise<string> {
  const key = threadMappingKey(paperId, agentType);
  const stored = getStoredThreadId(paperId, agentType);
  if (stored) return stored;

  const inflight = ensuring.get(key);
  if (inflight) return inflight;

  const p = (async () => {
    if (legacyMessages && legacyMessages.length > 0) {
      try {
        return await importThread({ agent_type: agentType, title, messages: legacyMessages });
      } catch (err) {
        if ((err as { status?: number }).status !== 404) throw err;
        return await createThread({ agent_type: agentType, title });
      }
    }
    return await createThread({ agent_type: agentType, title });
  })()
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
