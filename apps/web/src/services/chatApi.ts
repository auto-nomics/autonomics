/**
 * 聊天相关 API 客户端（autonomics chat blob）
 *
 * autonomics 的聊天持久化只有一个端点对：
 *   GET  /chat?scope={token} → {"payload": <JSON|null>}
 *   POST /chat {scope, payload}   （整体覆盖，POST 使 navigator.sendBeacon 可用）
 *
 * jayread 前端则有一整套「每篇论文多个会话」的模型（getMessages /
 * getConversations / createConversation / activateConversation / deleteConversation）。
 * 本模块在单个 JSON blob 之上**模拟**这套模型：
 *
 *   payload = {
 *     conversations: [{ id, title, created_at, updated_at, messages: [...] }],
 *     active_conversation_id: string | null
 *   }
 *
 * 关键约束：`messages[]` 里的对象是**不透明的**，必须原样往返。jayread 的
 * ChatMessage 带开放索引签名，运行时会塞进 `threads`（递归的追问线程）、
 * `agentType`、`blocks`、`quotedMessage` 等任意键，读回路径直接依赖这些键。
 * 后端对 payload 做 JSON 透传存储，所以这里绝不能对 message 做字段挑选或归一化。
 *
 * 为什么每次写都要先 GET 一次？blob 是整体覆盖，必须基于最新值做增量修改，
 * 否则两个标签页会互相覆盖丢消息。GET 用 `_t` cache-bust 绕开 client 的
 * 响应缓存（聊天状态绝不能读旧值）。
 */
import request, { API_BASE } from './client';
import { toSafeId, fromSafeId } from './mapping';
import type {
  Message,
  GetMessagesResponse,
  SaveMessagesResponse,
  ClearMessagesResponse,
  GetConversationsResponse,
  CreateConversationResponse,
  ActivateConversationResponse,
  DeleteConversationResponse,
  ConversationSummary,
  Conversation,
} from '@/types';

// ============================================================
// blob 模型
// ============================================================

/** blob 内的单个会话。messages 项不透明，原样往返。 */
interface BlobConversation {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  messages: unknown[];
}

/** autonomics `/chat` 端点里存的 payload 形状 */
interface ChatBlob {
  conversations: BlobConversation[];
  active_conversation_id: string | null;
}

/** 空会话兜底标题 */
function defaultConversationTitle(): string {
  const d = new Date();
  const pad = (n: number) => String(n).padStart(2, '0');
  return `新对话 ${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function newConversationId(): string {
  return `conv-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

// ============================================================
// scope
// ============================================================

/**
 * 构造 autonomics 的 scope token。
 *
 * - `standalone`：首页（无论文）的全局对话
 * - `article:{safeId}`：某篇论文的对话
 *
 * safeId 只含 [A-Za-z0-9_-]，不会与 scope 的 `article:` 前缀或分隔符冲突。
 * jayread 的 useChatInit 会传 `String(paperId)`，paperId 缺省时得到字面量
 * "undefined" —— 这里一并归到 standalone。
 */
function scopeFor(paperId: string | null | undefined): string {
  const id = paperId === null || paperId === undefined ? '' : String(paperId).trim();
  if (!id || id === 'undefined' || id === 'null') return 'standalone';
  // toSafeId(fromSafeId(id)) 把「真实 ID」和「safeId」两种输入归一到同一个 scope ——
  // getMessages 可能拿到 safeId（组件持有）而别处拿到真实 ID，两者必须命中同一条记录
  return `article:${toSafeId(fromSafeId(id))}`;
}

// ============================================================
// blob 读写
// ============================================================

/**
 * 每个 scope 最近一次读/写看到的 blob 镜像。
 *
 * sendBeacon 是同步 fire-and-forget，没法像 saveMessages 那样先 GET
 * 最新 blob 再改写；这里镜像 loadBlob/saveBlob 的结果，让 beacon 能
 * 同步构造出保真的 `{scope, payload}`。chat UI 挂载必然先 getMessages
 * （loadBlob）一次，镜像因此总是热的；未命中时退化为「只有一个会话」
 * 的新 blob —— 可接受，总比丢消息强。
 */
const blobMirror = new Map<string, ChatBlob>();

/** 读取 scope 的 payload；无记录返回 null。`_t` 绕开 client 缓存。 */
async function loadBlob(scope: string, signal?: AbortSignal): Promise<ChatBlob | null> {
  const data = await request<{ payload: ChatBlob | null }>(
    `/chat?scope=${encodeURIComponent(scope)}&_t=${Date.now()}`,
    { signal },
  );
  const payload = data?.payload;
  if (!payload || typeof payload !== 'object') return null;
  // 结构兜底：后端只做 JSON 透传，畸形/空对象都归一成合法 blob
  const blob: ChatBlob = {
    conversations: Array.isArray(payload.conversations) ? payload.conversations : [],
    active_conversation_id:
      typeof payload.active_conversation_id === 'string' ? payload.active_conversation_id : null,
  };
  blobMirror.set(scope, blob);
  return blob;
}

/** 整体覆盖写回 payload */
async function saveBlob(scope: string, payload: ChatBlob, signal?: AbortSignal): Promise<void> {
  await request('/chat', { method: 'POST', body: { scope, payload }, signal });
  blobMirror.set(scope, payload);
}

// ============================================================
// beforeunload sendBeacon 兜底
// ============================================================

/**
 * 同步构造 beforeunload sendBeacon 用的请求（不实际发送）。
 *
 * body 与 saveMessages 的写回形状完全一致：镜像 blob + 活跃会话的
 * messages 替换为传入值。拆出「构造」这一步是因为调用方
 * （useMessagePersistence）还要拿同一份 body 写 sessionStorage 做
 * 发送失败时的离线兜底。
 *
 * @returns null 表示消息为空，不值得发送
 */
export function buildChatBeaconSave(
  paperId: string | number | null | undefined,
  messages: unknown[],
): { url: string; body: string } | null {
  if (!messages || messages.length === 0) return null;
  const scope = scopeFor(paperId == null ? undefined : String(paperId));
  const base = blobMirror.get(scope) ?? { conversations: [], active_conversation_id: null };
  // 深拷贝：beacon 构造绝不能改写镜像本身
  const blob: ChatBlob = JSON.parse(JSON.stringify(base));
  const now = new Date().toISOString();
  let active = activeConversationOf(blob);
  if (!active) {
    active = {
      id: newConversationId(),
      title: defaultConversationTitle(),
      created_at: now,
      updated_at: now,
      messages: [],
    };
    blob.conversations.push(active);
  }
  active.messages = messages;
  active.updated_at = now;
  blob.active_conversation_id = active.id;
  return { url: `${API_BASE}/chat`, body: JSON.stringify({ scope, payload: blob }) };
}

/**
 * 重发一条 sessionStorage 里兜底的 beacon body（组件 mount 时调用）。
 * body 自带 scope，无需再传 paperId。
 *
 * @returns 是否发送成功（失败时调用方保留 sessionStorage 等下次重试）
 */
export async function postChatBeaconBody(body: string): Promise<boolean> {
  try {
    const parsed = JSON.parse(body) as { scope: string; payload: ChatBlob };
    await request('/chat', { method: 'POST', body: parsed });
    return true;
  } catch {
    return false;
  }
}

/** 取活跃会话；没有会话或 active 指向不存在的 id 时，落到最近更新的那个。 */
function activeConversationOf(blob: ChatBlob): BlobConversation | null {
  if (blob.conversations.length === 0) return null;
  const byId = blob.conversations.find((c) => c.id === blob.active_conversation_id);
  if (byId) return byId;
  return [...blob.conversations].sort((a, b) => String(b.updated_at).localeCompare(String(a.updated_at)))[0];
}

// ============================================================
// 消息
// ============================================================

/**
 * 获取指定论文的聊天历史记录（活跃会话的消息）
 *
 * @param {string} paperId - 论文 ID（safeId）；空/undefined 走 standalone scope
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<GetMessagesResponse>} 历史消息数组
 */
export async function getMessages(paperId: string, signal?: AbortSignal): Promise<GetMessagesResponse> {
  const blob = await loadBlob(scopeFor(paperId), signal);
  const active = blob ? activeConversationOf(blob) : null;
  // messages 项不做任何归一化 —— jayread 的 ensureThreadStructure 会在 UI 侧补
  // threads 字段，这里补反而会丢掉运行时塞进来的其他键
  return { messages: ((active?.messages ?? []) as unknown) as Message[] };
}

/**
 * 保存聊天记录（全量覆盖当前活跃会话的消息列表）
 *
 * 为什么是「读-改-写」三步？后端是整体覆盖，没有按会话写消息的端点，
 * 这里先读最新 blob、只改活跃会话的 messages、再整体写回。
 *
 * @param {string} paperId - 论文 ID（safeId）
 * @param {Array<Message>} messages - 完整的消息数组（含 threads 等运行时键，原样存储）
 * @returns {Promise<SaveMessagesResponse>} 保存确认消息
 */
export async function saveMessages(paperId: string, messages: Message[], signal?: AbortSignal): Promise<SaveMessagesResponse> {
  const scope = scopeFor(paperId);
  const blob = (await loadBlob(scope, signal)) ?? {
    conversations: [],
    active_conversation_id: null,
  };

  const now = new Date().toISOString();
  let active = activeConversationOf(blob);

  if (!active) {
    // 首次保存：blob 为空或没有任何会话，建一个
    active = {
      id: newConversationId(),
      title: defaultConversationTitle(),
      created_at: now,
      updated_at: now,
      messages: messages as unknown as unknown[],
    };
    blob.conversations.push(active);
    blob.active_conversation_id = active.id;
  } else {
    active.messages = messages as unknown as unknown[];
    active.updated_at = now;
    blob.active_conversation_id = active.id;
  }

  await saveBlob(scope, blob, signal);
  return { message: '已保存' };
}

/**
 * 清空指定论文当前会话的聊天记录
 *
 * 当前无线上调用方（保留以对齐签名）。后端没有「删除 scope」端点，
 * 等价实现是写入空消息数组。
 *
 * @param {string} paperId - 论文 ID（safeId）
 * @returns {Promise<ClearMessagesResponse>} 清空确认消息
 */
export async function clearMessages(paperId: string): Promise<ClearMessagesResponse> {
  await saveMessages(paperId, []);
  return { message: '已清空' };
}

// ============================================================
// 会话管理
// ============================================================

/**
 * 获取论文的所有对话会话列表
 *
 * 按更新时间倒序（最近活跃在前），`is_active` 标出当前活跃会话。
 *
 * @param {string} paperId - 论文 ID（safeId）
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<GetConversationsResponse>} 对话列表
 */
export async function getConversations(paperId: string, signal?: AbortSignal): Promise<GetConversationsResponse> {
  const blob = await loadBlob(scopeFor(paperId), signal);
  const conversations = blob?.conversations ?? [];
  const active = blob ? activeConversationOf(blob) : null;

  const mapped: Conversation[] = conversations
    .map((c) => ({
      id: c.id,
      title: c.title || defaultConversationTitle(),
      // jayread 的 Conversation.paper_id 在弹窗里没人读，填 safeId 便于调试
      paper_id: paperId,
      is_active: !!active && c.id === active.id,
      message_count: Array.isArray(c.messages) ? c.messages.length : 0,
      created_at: c.created_at,
      updated_at: c.updated_at,
    }))
    // 最近活跃在前，与 jayread 后端的排序一致。
    // 活跃会话强制排最前：同一毫秒内建的两个会话 updated_at 相同，纯按时间排
    // 会不稳定，弹窗里「当前会话」可能跑到第二行。
    .sort((a, b) => {
      if (a.is_active !== b.is_active) return a.is_active ? -1 : 1;
      return String(b.updated_at).localeCompare(String(a.updated_at));
    });

  return { conversations: mapped };
}

/**
 * 创建新的对话会话
 *
 * 旧会话保留（可通过 /resume 恢复），新会话置为活跃并设为空消息。
 *
 * @param {string} paperId - 论文 ID（safeId）
 * @returns {Promise<CreateConversationResponse>} 新对话 ID
 */
export async function createConversation(paperId: string): Promise<CreateConversationResponse> {
  const scope = scopeFor(paperId);
  const blob = (await loadBlob(scope)) ?? { conversations: [], active_conversation_id: null };

  const now = new Date().toISOString();
  const created: BlobConversation = {
    id: newConversationId(),
    title: defaultConversationTitle(),
    created_at: now,
    updated_at: now,
    messages: [],
  };

  blob.conversations.push(created);
  blob.active_conversation_id = created.id;
  await saveBlob(scope, blob);

  // jayread 类型把 conversation_id 标成 number（自增主键时代），autonomics 是字符串 id。
  // 类型定义待 Phase 5 清扫，这里 cast 保持导出签名不变。
  return { conversation_id: created.id as unknown as number, message: '已创建' };
}

/**
 * 切换到指定的历史对话
 *
 * 只改 active_conversation_id 指针，消息内容不动。
 *
 * @param {string} paperId - 论文 ID（safeId）
 * @param {string} conversationId - 要切换到的对话会话 ID
 * @returns {Promise<ActivateConversationResponse>} 切换结果
 */
export async function activateConversation(paperId: string, conversationId: string): Promise<ActivateConversationResponse> {
  const scope = scopeFor(paperId);
  const blob = await loadBlob(scope);

  if (!blob || !blob.conversations.some((c) => c.id === conversationId)) {
    // 目标不存在：静默当作无操作，调用方随后 getMessages 会得到空列表
    return { conversation_id: conversationId as unknown as number, message: '会话不存在' };
  }

  blob.active_conversation_id = conversationId;
  await saveBlob(scope, blob);
  return { conversation_id: conversationId as unknown as number, message: '已切换' };
}

/**
 * 删除指定的对话会话
 *
 * jayread 后端保证「删完至少剩一个活跃会话」，这里复刻该不变式：
 * 删掉的是活跃会话时切到最近更新的剩余会话，一个不剩时新建空会话。
 *
 * @param {string} paperId - 论文 ID（safeId）
 * @param {string} conversationId - 要删除的对话会话 ID
 * @returns {Promise<DeleteConversationResponse>} 删除结果
 */
export async function deleteConversation(paperId: string, conversationId: string): Promise<DeleteConversationResponse> {
  const scope = scopeFor(paperId);
  const blob = await loadBlob(scope);
  if (!blob) return { message: '已删除' };

  blob.conversations = blob.conversations.filter((c) => c.id !== conversationId);

  if (blob.conversations.length === 0) {
    const now = new Date().toISOString();
    const fresh: BlobConversation = {
      id: newConversationId(),
      title: defaultConversationTitle(),
      created_at: now,
      updated_at: now,
      messages: [],
    };
    blob.conversations.push(fresh);
    blob.active_conversation_id = fresh.id;
  } else if (blob.active_conversation_id === conversationId) {
    const next = [...blob.conversations].sort((a, b) =>
      String(b.updated_at).localeCompare(String(a.updated_at)))[0];
    blob.active_conversation_id = next.id;
  }

  await saveBlob(scope, blob);
  return { message: '已删除' };
}

// ============================================================
// 摘要（桩）
// ============================================================

/**
 * 生成对话历史的渐进式摘要
 *
 * ⚠️ 桩：autonomics 无独立的摘要端点。这里退化为**本地摘要**——把被压缩的
 * 旧消息截取成一段纯文本，塞回 `[上下文摘要]` 消息里。
 *
 * 为什么不做纯 reject？唯一调用方 messageCompaction 对失败直接 throw
 *（`对话压缩失败`），会把整条消息发送打断。本地摘要在功能上是降级
 *（不是 AI 语义摘要）但链路不断：旧上下文仍以前缀形式保留，只是更粗糙。
 *
 * 当前主对话链路 `compactHistory(..., isAgentMode=true)` 会整体跳过压缩，
 * 此函数实际不会被调用，是给未来切换留的兜底。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 4/5)
 */
export async function summarizeConversation(
  paperId: string,
  messages: Message[],
  _paperTitle: string,
  _signal?: AbortSignal,
): Promise<ConversationSummary> {
  void paperId; void _paperTitle; void _signal;

  const lines: string[] = [];
  for (const m of messages) {
    const text = typeof m.content === 'string' ? m.content : JSON.stringify(m.content);
    if (!text) continue;
    // 每条截 200 字符，整段封顶 4000 字符，避免摘要本身膨胀到需要再次压缩
    lines.push(`${m.role}: ${text.length > 200 ? `${text.slice(0, 200)}…` : text}`);
    if (lines.join('\n').length > 4000) break;
  }

  return {
    summary: `（本地截断摘要，autonomics 暂无 AI 摘要能力）\n\n${lines.join('\n')}`,
    summarized_count: messages.length,
  };
}
