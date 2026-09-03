import { describe, it, expect, beforeEach } from 'vitest';
import { useChatStore, getMessagesForPaper, getStreamingForPaper } from '../useChatStore';
import { DEFAULT_TOKEN_BUDGETS } from '../../features/ai-chat/utils/chatConstants';

describe('useChatStore — settingsSlice + modelConfigSlice', () => {
  beforeEach(() => {
    // 每个用例前重置 store 到初始状态，避免用例间串话
    useChatStore.setState({
      settings: {},
      webSearchEnabled: false,
      threadContextInjection: false,
      tokenBudgets: DEFAULT_TOKEN_BUDGETS,
      customSystemPrompts: {} as Record<string, string>,
      currentModel: null,
      customConfigs: [],
      selectedConfigId: null,
      messagesByPaperId: {},
      streamingByPaperId: {},
    });
  });

  describe('初始状态', () => {
    it('settings 默认空对象', () => {
      expect(useChatStore.getState().settings).toEqual({});
    });

    it('webSearchEnabled / threadContextInjection 默认 false', () => {
      const s = useChatStore.getState();
      expect(s.webSearchEnabled).toBe(false);
      expect(s.threadContextInjection).toBe(false);
    });

    it('tokenBudgets 默认等于 DEFAULT_TOKEN_BUDGETS', () => {
      expect(useChatStore.getState().tokenBudgets).toBe(DEFAULT_TOKEN_BUDGETS);
    });

    it('currentModel / customConfigs / selectedConfigId 默认空', () => {
      const s = useChatStore.getState();
      expect(s.currentModel).toBeNull();
      expect(s.customConfigs).toEqual([]);
      expect(s.selectedConfigId).toBeNull();
    });
  });

  describe('settingsSlice setters', () => {
    it('setSettings 替换整个 settings 对象', () => {
      useChatStore.getState().setSettings({ api_key: 'xxx', model: 'gpt-4o' });
      expect(useChatStore.getState().settings).toEqual({ api_key: 'xxx', model: 'gpt-4o' });
    });

    it('setWebSearchEnabled 切换 boolean', () => {
      useChatStore.getState().setWebSearchEnabled(true);
      expect(useChatStore.getState().webSearchEnabled).toBe(true);
    });

    it('setThreadContextInjection 切换 boolean', () => {
      useChatStore.getState().setThreadContextInjection(true);
      expect(useChatStore.getState().threadContextInjection).toBe(true);
    });

    it('setTokenBudgets 替换整个字典', () => {
      const next = { ...DEFAULT_TOKEN_BUDGETS, 'gpt-4': 9999 };
      useChatStore.getState().setTokenBudgets(next);
      expect(useChatStore.getState().tokenBudgets['gpt-4']).toBe(9999);
    });

    it('setCustomSystemPrompts 按 agentType 分桶写入', () => {
      const next: Record<string, string> = { homepage: 'hi', paperReader: 'read' };
      useChatStore.getState().setCustomSystemPrompts(next as any);
      expect(useChatStore.getState().customSystemPrompts).toEqual(next);
    });
  });

  describe('modelConfigSlice setters', () => {
    it('setCurrentModel 写入完整对象', () => {
      const m = { key: 'custom', label: '自定义', configId: 'c1', config: undefined };
      useChatStore.getState().setCurrentModel(m);
      expect(useChatStore.getState().currentModel).toEqual(m);
    });

    it('setCustomConfigs 替换列表', () => {
      const cfgs = [
        { id: 'c1', name: '配置1', modelName: 'gpt-4o', baseUrl: '', apiKey: '', apiFormat: 'openai' },
      ];
      useChatStore.getState().setCustomConfigs(cfgs);
      expect(useChatStore.getState().customConfigs).toHaveLength(1);
      expect(useChatStore.getState().customConfigs[0].id).toBe('c1');
    });

    it('setSelectedConfigId 设置 / 清空', () => {
      useChatStore.getState().setSelectedConfigId('c1');
      expect(useChatStore.getState().selectedConfigId).toBe('c1');
      useChatStore.getState().setSelectedConfigId(null);
      expect(useChatStore.getState().selectedConfigId).toBeNull();
    });
  });

  describe('跨 slice 隔离', () => {
    it('修改 settings 不影响 modelConfig 状态', () => {
      useChatStore.getState().setCurrentModel({ key: 'gpt-4o', label: 'GPT' });
      useChatStore.getState().setSettings({ api_key: 'k' });
      const s = useChatStore.getState();
      expect(s.settings).toEqual({ api_key: 'k' });
      expect(s.currentModel).toEqual({ key: 'gpt-4o', label: 'GPT' });
    });
  });
});

describe('useChatStore — messagesSlice (per-paperId)', () => {
  beforeEach(() => {
    useChatStore.setState({ messagesByPaperId: {} });
  });

  describe('初始状态', () => {
    it('messagesByPaperId 默认空对象', () => {
      expect(useChatStore.getState().messagesByPaperId).toEqual({});
    });

    it('getMessagesForPaper 对未知 paperId 返回空数组', () => {
      expect(getMessagesForPaper('never-set')).toEqual([]);
    });

    it('getMessagesForPaper(undefined) 落到 __default__ 桶', () => {
      useChatStore.getState().setMessagesForPaper(undefined, [
        { role: 'user', content: 'hi' },
      ]);
      expect(getMessagesForPaper(undefined)).toHaveLength(1);
      expect(getMessagesForPaper(null)).toHaveLength(1); // null 也走 __default__
    });
  });

  describe('setMessagesForPaper', () => {
    it('直接传值：覆盖整个数组', () => {
      const msgs = [
        { role: 'user' as const, content: 'hello' },
        { role: 'assistant' as const, content: 'hi back' },
      ];
      useChatStore.getState().setMessagesForPaper('paper-1', msgs);
      expect(getMessagesForPaper('paper-1')).toEqual(msgs);
    });

    it('updater 函数：基于 prev 计算 next', () => {
      useChatStore.getState().setMessagesForPaper('paper-1', [
        { role: 'user', content: 'first' },
      ]);
      useChatStore.getState().setMessagesForPaper('paper-1', (prev) => [
        ...prev,
        { role: 'assistant', content: 'second' },
      ]);
      expect(getMessagesForPaper('paper-1')).toHaveLength(2);
      expect(getMessagesForPaper('paper-1')[1]).toEqual({ role: 'assistant', content: 'second' });
    });

    it('updater 函数：对未初始化的 paperId，prev 默认为空数组', () => {
      useChatStore.getState().setMessagesForPaper('fresh', (prev) => [
        ...prev,
        { role: 'user', content: 'first msg' },
      ]);
      expect(getMessagesForPaper('fresh')).toEqual([{ role: 'user', content: 'first msg' }]);
    });

    it('number 类型 paperId 与 string 类型不串话', () => {
      useChatStore.getState().setMessagesForPaper(123, [{ role: 'user', content: 'num' }]);
      useChatStore.getState().setMessagesForPaper('123', [{ role: 'user', content: 'str' }]);
      // number 123 → key '123'，string '123' → key '123'，落在同一桶
      // 这是预期行为：paperId 在 jayread 体系里始终序列化为 string
      expect(getMessagesForPaper(123)[0].content).toBe('str');
    });

    it('多 paperId 互不串话', () => {
      useChatStore.getState().setMessagesForPaper('a', [{ role: 'user', content: 'A' }]);
      useChatStore.getState().setMessagesForPaper('b', [{ role: 'user', content: 'B' }]);
      expect(getMessagesForPaper('a')).toHaveLength(1);
      expect(getMessagesForPaper('a')[0].content).toBe('A');
      expect(getMessagesForPaper('b')[0].content).toBe('B');
    });
  });

  describe('appendMessage', () => {
    it('追加到指定 paperId 尾部', () => {
      useChatStore.getState().setMessagesForPaper('p1', [{ role: 'user', content: 'a' }]);
      useChatStore.getState().appendMessage('p1', { role: 'user', content: 'b' });
      expect(getMessagesForPaper('p1')).toHaveLength(2);
      expect(getMessagesForPaper('p1')[1].content).toBe('b');
    });

    it('对未初始化 paperId 也能工作（自动建桶）', () => {
      useChatStore.getState().appendMessage('fresh', { role: 'user', content: 'first' });
      expect(getMessagesForPaper('fresh')).toEqual([{ role: 'user', content: 'first' }]);
    });
  });

  describe('clearPaper', () => {
    it('清掉指定 paperId 的消息桶', () => {
      useChatStore.getState().setMessagesForPaper('to-clear', [{ role: 'user', content: 'x' }]);
      useChatStore.getState().setMessagesForPaper('keep', [{ role: 'user', content: 'y' }]);
      useChatStore.getState().clearPaper('to-clear');
      expect(getMessagesForPaper('to-clear')).toEqual([]);
      expect(getMessagesForPaper('keep')).toHaveLength(1);
    });

    it('清不存在的 paperId 是 no-op（不抛错）', () => {
      expect(() => useChatStore.getState().clearPaper('never-set')).not.toThrow();
    });
  });
});

describe('useChatStore — streamingSlice (per-paperId)', () => {
  beforeEach(() => {
    useChatStore.setState({ streamingByPaperId: {}, messagesByPaperId: {} });
  });

  describe('初始状态', () => {
    it('streamingByPaperId 默认空对象', () => {
      expect(useChatStore.getState().streamingByPaperId).toEqual({});
    });

    it('getStreamingForPaper 对未知 paperId 返回 false', () => {
      expect(getStreamingForPaper('never-set')).toBe(false);
    });

    it('getStreamingForPaper(undefined) 落到 __default__ 桶', () => {
      useChatStore.getState().setStreamingForPaper(undefined, true);
      expect(getStreamingForPaper(undefined)).toBe(true);
      expect(getStreamingForPaper(null)).toBe(true);
    });
  });

  describe('setStreamingForPaper', () => {
    it('设置 / 切换 / 重置 flag', () => {
      useChatStore.getState().setStreamingForPaper('p1', true);
      expect(getStreamingForPaper('p1')).toBe(true);
      useChatStore.getState().setStreamingForPaper('p1', false);
      expect(getStreamingForPaper('p1')).toBe(false);
    });

    it('多 paperId 互不串话', () => {
      useChatStore.getState().setStreamingForPaper('a', true);
      useChatStore.getState().setStreamingForPaper('b', false);
      expect(getStreamingForPaper('a')).toBe(true);
      expect(getStreamingForPaper('b')).toBe(false);
    });
  });

  describe('clearPaper 同时清 streaming', () => {
    it('clearPaper 后 streaming flag 也清掉', () => {
      useChatStore.getState().setStreamingForPaper('p1', true);
      useChatStore.getState().setMessagesForPaper('p1', [{ role: 'user', content: 'x' }]);
      useChatStore.getState().clearPaper('p1');
      expect(getStreamingForPaper('p1')).toBe(false);
      expect(getMessagesForPaper('p1')).toEqual([]);
    });
  });
});
