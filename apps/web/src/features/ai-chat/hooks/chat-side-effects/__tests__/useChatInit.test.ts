/**
 * useChatInit P4 预导入测试
 *
 * runtime 模式 + bib 有历史 + 无 thread 映射 → 打开面板即经
 * ensureMainThreadSession 把旧对话迁入服务端（不阻塞 loading，尽力而为）。
 * 其余组合（ephemeral / 空历史 / 已有映射）不触发预导入。
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';

const probeAgentMode = vi.fn();
const getStoredThreadId = vi.fn();
const ensureMainThreadSession = vi.fn();
const fetchThreadMessages = vi.fn();

vi.mock('../../../api/agentThreadsApi', () => ({
  probeAgentMode: (...args: unknown[]) => probeAgentMode(...args),
  getStoredThreadId: (...args: unknown[]) => getStoredThreadId(...args),
  ensureMainThreadSession: (...args: unknown[]) => ensureMainThreadSession(...args),
  fetchThreadMessages: (...args: unknown[]) => fetchThreadMessages(...args),
  // 归一逻辑有自己的单测；这里做透传便于断言入参
  toImportMessages: (msgs: Array<{ role: string; content: string }>) =>
    msgs.filter((m) => m.role === 'user' || m.role === 'assistant'),
}));

const getMessages = vi.fn();
const getSettings = vi.fn();

vi.mock('../../../../../services/chatApi', () => ({
  getMessages: (...args: unknown[]) => getMessages(...args),
}));
vi.mock('../../../../../services/settingsApi', () => ({
  getSettings: (...args: unknown[]) => getSettings(...args),
}));

import { useChatInit } from '../useChatInit';

function createProps(overrides: Record<string, unknown> = {}) {
  return {
    paperId: 'p1',
    agentType: 'paperReader',
    setMessages: vi.fn(),
    setSettings: vi.fn(),
    setStreaming: vi.fn(),
    initializeCustomConfigs: vi.fn(),
    loadTemplates: vi.fn(),
    abortControllerRef: { current: null },
    activeRequestIdRef: { current: null },
    ...overrides,
  };
}

describe('useChatInit：P4 旧对话预导入', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    probeAgentMode.mockResolvedValue('ephemeral');
    getStoredThreadId.mockReturnValue(null);
    ensureMainThreadSession.mockResolvedValue('t-imported');
    fetchThreadMessages.mockResolvedValue(null);
    getMessages.mockResolvedValue({ messages: [] });
    getSettings.mockResolvedValue({ settings: {} });
  });

  it('runtime + bib 有历史 + 无映射 → 预导入旧对话（不阻塞 loading）', async () => {
    probeAgentMode.mockResolvedValue('runtime');
    getMessages.mockResolvedValue({
      messages: [
        { role: 'user', content: '旧问题', id: 'm1' },
        { role: 'assistant', content: '旧回答', id: 'm2' },
      ],
    });

    const props = createProps();
    const { result } = renderHook(() => useChatInit(props as any));

    // loading 正常结束：预导入是 fire-and-forget，不在加载关键路径上
    await waitFor(() => expect(result.current.loading).toBe(false));
    await waitFor(() => expect(ensureMainThreadSession).toHaveBeenCalledTimes(1));

    expect(ensureMainThreadSession).toHaveBeenCalledWith(
      'p1',
      'paperReader',
      undefined,
      // ensureThreadStructure 会给每条消息补 threads: []（与生产一致）
      [
        { role: 'user', content: '旧问题', id: 'm1', threads: [] },
        { role: 'assistant', content: '旧回答', id: 'm2', threads: [] },
      ],
    );
  });

  it('预导入失败静默（首条发送会重试并在那里报错）', async () => {
    probeAgentMode.mockResolvedValue('runtime');
    getMessages.mockResolvedValue({ messages: [{ role: 'user', content: '旧问题', id: 'm1' }] });
    ensureMainThreadSession.mockRejectedValue(new Error('backend down'));

    const props = createProps();
    const { result } = renderHook(() => useChatInit(props as any));

    await waitFor(() => expect(result.current.loading).toBe(false));
    // 未捕获拒绝不冒泡：等一拍确保 catch 生效即可
    await waitFor(() => expect(ensureMainThreadSession).toHaveBeenCalled());
    expect(props.setMessages).toHaveBeenCalled();
  });

  it('ephemeral 模式不预导入', async () => {
    probeAgentMode.mockResolvedValue('ephemeral');
    getMessages.mockResolvedValue({ messages: [{ role: 'user', content: '旧问题', id: 'm1' }] });

    const props = createProps();
    const { result } = renderHook(() => useChatInit(props as any));
    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(ensureMainThreadSession).not.toHaveBeenCalled();
  });

  it('bib 空历史不预导入（P2 水合路径负责，互斥）', async () => {
    probeAgentMode.mockResolvedValue('runtime');
    getMessages.mockResolvedValue({ messages: [] });

    const props = createProps();
    const { result } = renderHook(() => useChatInit(props as any));
    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(ensureMainThreadSession).not.toHaveBeenCalled();
  });

  it('已有映射不预导入（直接复用服务端会话）', async () => {
    probeAgentMode.mockResolvedValue('runtime');
    getStoredThreadId.mockReturnValue('t-existing');
    getMessages.mockResolvedValue({ messages: [{ role: 'user', content: '旧问题', id: 'm1' }] });

    const props = createProps();
    const { result } = renderHook(() => useChatInit(props as any));
    await waitFor(() => expect(result.current.loading).toBe(false));

    expect(ensureMainThreadSession).not.toHaveBeenCalled();
  });
});
