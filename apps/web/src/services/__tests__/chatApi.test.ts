/**
 * chatApi 测试（autonomics chat blob 契约）
 *
 * autonomics 只有一个 blob 端点对（GET/POST /chat?scope=...），本模块在其上
 * 模拟 jayread 的多会话模型。测试重点：
 * - scope 构造（standalone / article:{safeId}）
 * - payload 里 messages 的**不透明往返**（threads 等运行时键绝不能被剥掉）
 * - 会话的创建 / 切换 / 删除不变式（永远至少剩一个活跃会话）
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import {
  getMessages,
  saveMessages,
  clearMessages,
  getConversations,
  createConversation,
  activateConversation,
  deleteConversation,
  summarizeConversation,
} from '../chatApi';
import { toSafeId } from '../mapping';
import { invalidateCache } from '../client';

const REAL_ID = 'doi:10.1000/xyz';
const SCOPE_ARTICLE = `article:${toSafeId(REAL_ID)}`;

/** 模拟后端的 blob 存储 */
function makeBlobStore(initial: Record<string, unknown> = {}) {
  const store: Record<string, unknown> = { ...initial };
  return {
    store,
    fetch: vi.fn((url: string, init?: RequestInit) => {
      const u = String(url).replace(/^\/api\/v1\/bib/, '');
      if (u.startsWith('/chat?') && (!init?.method || init.method === 'GET')) {
        const scope = decodeURIComponent(u.split('scope=')[1].split('&')[0]);
        const payload = Object.prototype.hasOwnProperty.call(store, scope) ? store[scope] : null;
        return Promise.resolve(jsonResponse({ payload: structuredCloneSafe(payload) }));
      }
      if (u === '/chat' && init?.method === 'POST') {
        const body = JSON.parse(init.body as string);
        store[body.scope] = structuredCloneSafe(body.payload);
        return Promise.resolve(jsonResponse({ payload: body.payload }));
      }
      return Promise.resolve(jsonResponse({}, false, 404));
    }),
  };
}

function structuredCloneSafe<T>(v: T): T {
  return v === null || v === undefined ? v : (JSON.parse(JSON.stringify(v)) as T);
}

function jsonResponse(body: unknown, ok = true, status = 200) {
  return {
    ok,
    status,
    statusText: ok ? 'OK' : 'Error',
    headers: { get: () => 'application/json' },
    json: () => Promise.resolve(body),
  };
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
// scope
// ============================================================================

describe('scope 构造', () => {
  it('论文对话用 article:{safeId}', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    await getMessages(REAL_ID);

    const url = blob.fetch.mock.calls[0][0] as string;
    expect(decodeURIComponent(url.split('scope=')[1].split('&')[0])).toBe(SCOPE_ARTICLE);
  });

  it('接受 safeId 输入（组件传来的形态）', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    await getMessages(toSafeId(REAL_ID));

    const url = blob.fetch.mock.calls[0][0] as string;
    expect(decodeURIComponent(url.split('scope=')[1].split('&')[0])).toBe(SCOPE_ARTICLE);
  });

  it('空 / undefined / "undefined" 归到 standalone（首页全局对话）', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    await getMessages('');
    await getMessages('undefined');
    await getMessages('null');

    const scopes = blob.fetch.mock.calls.map(
      (c) => decodeURIComponent(String(c[0]).split('scope=')[1].split('&')[0]),
    );
    expect(scopes).toEqual(['standalone', 'standalone', 'standalone']);
  });
});

// ============================================================================
// 消息读写
// ============================================================================

describe('getMessages / saveMessages', () => {
  it('无记录时返回空消息列表', async () => {
    global.fetch = makeBlobStore().fetch as unknown as typeof fetch;

    const res = await getMessages(REAL_ID);
    expect(res).toEqual({ messages: [] });
  });

  it('返回 payload 为 null 时也返回空列表（后端 404 场景）', async () => {
    global.fetch = makeBlobStore().fetch as unknown as typeof fetch;

    await expect(getMessages(REAL_ID)).resolves.toEqual({ messages: [] });
  });

  it('saveMessages 后 getMessages 能读回同样的消息', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    const messages = [
      { id: 'm1', role: 'user', content: '这篇论文的方法是什么？' },
      { id: 'm2', role: 'assistant', content: '作者用了 X 方法。' },
    ];
    await saveMessages(REAL_ID, messages as never);

    const { messages: out } = await getMessages(REAL_ID);
    expect(out).toHaveLength(2);
    expect(out[0].content).toBe('这篇论文的方法是什么？');
  });

  it('messages 是不透明载荷：threads 等运行时键原样往返', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    // jayread 的 ChatMessage 带开放索引签名，threads 是递归的追问线程
    const messages = [
      {
        id: 'm1',
        role: 'user',
        content: '正文',
        agentType: 'paperReader',
        paperId: 'ignored-at-service-level',
        threads: [
          {
            id: 't1',
            anchorText: '选中的原文',
            anchorFingerprint: 'fp-1',
            orphaned: false,
            messages: [{ id: 'tm1', role: 'assistant', content: '追问回复', threads: [] }],
          },
        ],
      },
    ];
    await saveMessages(REAL_ID, messages as never);

    const { messages: out } = await getMessages(REAL_ID) as unknown as { messages: Array<Record<string, unknown>> };
    const threads = out[0].threads as Array<Record<string, unknown>>;
    expect(out[0].agentType).toBe('paperReader');
    expect(threads).toHaveLength(1);
    expect(threads[0].anchorFingerprint).toBe('fp-1');
    // 递归层级也要保留
    expect((threads[0].messages as Array<Record<string, unknown>>)[0].id).toBe('tm1');
  });

  it('保存走读-改-写：先 GET 最新 blob 再 POST 整体覆盖', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    await saveMessages(REAL_ID, [{ id: 'm1', role: 'user', content: 'hi' }] as never);

    expect(blob.fetch.mock.calls[0][1]?.method).toBeUndefined(); // GET
    expect(blob.fetch.mock.calls[1][1]?.method).toBe('POST');     // POST
    const postBody = JSON.parse((blob.fetch.mock.calls[1][1] as RequestInit).body as string);
    expect(Object.keys(postBody).sort()).toEqual(['payload', 'scope']);
    expect(postBody.scope).toBe(SCOPE_ARTICLE);
    expect(postBody.payload.conversations).toHaveLength(1);
  });

  it('保存到已有会话时不新建第二个会话', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    await saveMessages(REAL_ID, [{ id: 'm1', role: 'user', content: 'a' }] as never);
    await saveMessages(REAL_ID, [
      { id: 'm1', role: 'user', content: 'a' },
      { id: 'm2', role: 'assistant', content: 'b' },
    ] as never);

    const { conversations } = await getConversations(REAL_ID);
    expect(conversations).toHaveLength(1);
    expect(conversations[0].message_count).toBe(2);
  });

  it('GET /chat 不命中 client 缓存（TTL 0），连续保存基于最新值', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    await saveMessages(REAL_ID, [{ id: 'm1', role: 'user', content: 'a' }] as never);
    await saveMessages(REAL_ID, [
      { id: 'm1', role: 'user', content: 'a' },
      { id: 'm2', role: 'assistant', content: 'b' },
    ] as never);

    // 第 0、2 个调用是两次保存各自的 GET（读最新 blob），都必须真实发出
    expect(blob.fetch).toHaveBeenCalledTimes(4);
    expect(String(blob.fetch.mock.calls[0][0])).toContain('/chat?scope=');
    expect(String(blob.fetch.mock.calls[2][0])).toContain('/chat?scope=');
  });
});

describe('clearMessages', () => {
  it('写入空消息数组（后端没有「删除 scope」端点）', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    await saveMessages(REAL_ID, [{ id: 'm1', role: 'user', content: 'a' }] as never);
    const res = await clearMessages(REAL_ID);

    expect(res.message).toBeTruthy();
    const { messages } = await getMessages(REAL_ID);
    expect(messages).toEqual([]);
  });
});

// ============================================================================
// 会话管理
// ============================================================================

describe('会话管理', () => {
  it('getConversations 标出活跃会话并按更新时间倒序', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    await saveMessages(REAL_ID, [{ id: 'm1', role: 'user', content: 'first' }] as never);
    await createConversation(REAL_ID); // 新会话变活跃

    const { conversations } = await getConversations(REAL_ID);
    expect(conversations).toHaveLength(2);
    expect(conversations[0].is_active).toBe(true);   // 最近更新的排最前
    expect(conversations[1].is_active).toBe(false);
    expect(conversations[0].message_count).toBe(0);  // 新会话为空
    expect(conversations[1].message_count).toBe(1);
    expect(conversations[1].title).toBeTruthy();
  });

  it('createConversation 返回新 id，旧会话保留', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    await saveMessages(REAL_ID, [{ id: 'm1', role: 'user', content: 'first' }] as never);
    const res = await createConversation(REAL_ID);

    expect(typeof res.conversation_id).toBe('string');
    const { conversations } = await getConversations(REAL_ID);
    expect(conversations).toHaveLength(2);
    expect(conversations.find((c) => c.id === (res.conversation_id as unknown as string))?.is_active).toBe(true);
  });

  it('activateConversation 切换活跃指针，消息不动', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    await saveMessages(REAL_ID, [{ id: 'm1', role: 'user', content: 'first' }] as never);
    const first = (await getConversations(REAL_ID)).conversations[0];
    const created = await createConversation(REAL_ID);
    await activateConversation(REAL_ID, created.conversation_id as unknown as string);

    const { messages } = await getMessages(REAL_ID);
    expect(messages).toEqual([]); // 新会话

    // 切回去能拿到旧消息
    await activateConversation(REAL_ID, first.id);
    const { messages: restored } = await getMessages(REAL_ID);
    expect(restored).toHaveLength(1);
  });

  it('activateConversation 目标不存在时静默无操作（不抛错）', async () => {
    global.fetch = makeBlobStore().fetch as unknown as typeof fetch;

    await expect(activateConversation(REAL_ID, 'nope')).resolves.toBeTruthy();
  });

  it('deleteConversation 移除会话', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    await saveMessages(REAL_ID, [{ id: 'm1', role: 'user', content: 'a' }] as never);
    const created = await createConversation(REAL_ID);
    await deleteConversation(REAL_ID, created.conversation_id as unknown as string);

    const { conversations } = await getConversations(REAL_ID);
    expect(conversations).toHaveLength(1);
  });

  it('删除活跃会话时自动切到最近更新的剩余会话', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    await saveMessages(REAL_ID, [{ id: 'm1', role: 'user', content: 'original' }] as never);
    const original = (await getConversations(REAL_ID)).conversations[0];
    const created = await createConversation(REAL_ID);

    await deleteConversation(REAL_ID, created.conversation_id as unknown as string);

    const { messages } = await getMessages(REAL_ID);
    expect(messages).toHaveLength(1); // 落回 original
    void original;
  });

  it('删除最后一个会话时新建一个空会话（jayread 的不变式）', async () => {
    const blob = makeBlobStore();
    global.fetch = blob.fetch as unknown as typeof fetch;

    await saveMessages(REAL_ID, [{ id: 'm1', role: 'user', content: 'only' }] as never);
    const only = (await getConversations(REAL_ID)).conversations[0];

    await deleteConversation(REAL_ID, only.id);

    const { conversations } = await getConversations(REAL_ID);
    expect(conversations).toHaveLength(1);
    expect(conversations[0].id).not.toBe(only.id);
    expect(conversations[0].is_active).toBe(true);
    expect(conversations[0].message_count).toBe(0);
  });
});

// ============================================================================
// summarizeConversation（桩）
// ============================================================================

describe('summarizeConversation — 本地摘要桩', () => {
  it('不发起网络请求，返回 {summary, summarized_count}', async () => {
    const f = vi.fn();
    global.fetch = f as unknown as typeof fetch;

    const messages = [
      { id: '1', role: 'user', content: '问题一' },
      { id: '2', role: 'assistant', content: '回答一' },
    ] as never;

    const res = await summarizeConversation(REAL_ID, messages, 'Some Title');

    expect(f).not.toHaveBeenCalled();
    expect(res.summarized_count).toBe(2);
    expect(typeof res.summary).toBe('string');
    expect(res.summary.length).toBeGreaterThan(0);
  });

  it('摘要包含消息角色与内容（链路不断，仅降级为截断摘要）', async () => {
    global.fetch = vi.fn() as unknown as typeof fetch;

    const res = await summarizeConversation(REAL_ID, [
      { id: '1', role: 'user', content: '问题一' },
    ] as never, 'T');

    expect(res.summary).toContain('user');
    expect(res.summary).toContain('问题一');
  });

  it('非字符串 content（多模态数组）序列化后不崩', async () => {
    global.fetch = vi.fn() as unknown as typeof fetch;

    const res = await summarizeConversation(REAL_ID, [
      { id: '1', role: 'user', content: [{ type: 'text', text: '看图' }] },
    ] as never, 'T');

    expect(res.summary).toBeTruthy();
  });
});
