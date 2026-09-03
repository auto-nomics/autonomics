/**
 * useChatStore — AI Chat 模块共享状态（zustand）
 *
 * Phase 4 引入：把原 ChatPanel 内的 useState 们提到全局 store，消除 prop drilling
 * 与 hook 之间的隐式时序耦合。
 *
 * 设计要点：
 * - **flat 结构**：不引入 slice composition（create(a, b, ...)）
 * - **pure setters only**：store action 只做 set({...})，不调 API、不弹 antd message
 *   副作用（updateSettings/checkTavilyKey）继续留在 useChatSettings/useModelConfig hook 里
 * - **type-only imports from ai-chat**：避免 store ↔ feature 循环依赖（types 在编译期擦除）
 * - **hybrid global/per-paper**：settings/modelConfig 是 global；messages 按 paperId 分桶
 *
 * @module stores/useChatStore
 */

import { create } from 'zustand';
import { DEFAULT_TOKEN_BUDGETS } from '../features/ai-chat/utils/chatConstants';
import type { AgentType } from '../features/ai-chat/agentTypes';
import type { ModelSelection, CustomModelConfig } from '../features/ai-chat/hooks/useModelConfig';

/**
 * 单条聊天消息（最小通过类型；feature 内部仍用各自的 ChatMessage 接口，
 * 但都 structurally compatible）。类型定义已迁移至 `@/types/chat`。
 */
import type { ChatMessage } from '@/types/chat';
export type { ChatMessage };

/** 把 paperId 归一成 store key（number / undefined / null 都能映射到稳定字符串） */
function paperKey(paperId: string | number | undefined | null): string {
  if (paperId === undefined || paperId === null) return '__default__';
  return String(paperId);
}

/** settingsSlice 状态 + actions（global） */
interface SettingsSlice {
  /** 用户设置原始对象（从后端 /api/settings 加载，含 api_key/model_config 等） */
  settings: Record<string, any>;
  /** Web 搜索开关（持久化到 settings.web_search_enabled） */
  webSearchEnabled: boolean;
  /** 线程上下文注入开关（持久化到 settings.thread_context_injection） */
  threadContextInjection: boolean;
  /** Token 预算字典（按 model 名映射 context window 大小） */
  tokenBudgets: typeof DEFAULT_TOKEN_BUDGETS;
  /** 按 agentType 分桶的自定义系统提示词（持久化为 JSON 字符串到 settings.custom_system_prompts） */
  customSystemPrompts: Record<AgentType, string>;

  setSettings: (s: Record<string, any>) => void;
  setWebSearchEnabled: (b: boolean) => void;
  setThreadContextInjection: (b: boolean) => void;
  setTokenBudgets: (b: typeof DEFAULT_TOKEN_BUDGETS) => void;
  setCustomSystemPrompts: (p: Record<AgentType, string>) => void;
}

/** modelConfigSlice 状态 + actions（global） */
interface ModelConfigSlice {
  /** 当前选中的模型（key='custom' 时 config 字段携带完整 CustomModelConfig） */
  currentModel: ModelSelection | null;
  /** 用户保存的自定义模型配置列表 */
  customConfigs: CustomModelConfig[];
  /** 当前选中的自定义配置 ID（仅 currentModel.key==='custom' 时有效） */
  selectedConfigId: string | null;

  setCurrentModel: (m: ModelSelection | null) => void;
  setCustomConfigs: (c: CustomModelConfig[]) => void;
  setSelectedConfigId: (id: string | null) => void;
}

/**
 * messagesSlice 状态 + actions（per-paperId）
 *
 * 为什么 per-paperId 而不是 global？
 * - 用户可能同时打开多个 PaperReader tab，每个 paperId 有独立的对话历史
 * - 切换 paper 时不应丢失上一个 paper 的未持久化输入
 * - 与 streaming/ui slice 共享同一 key 空间，便于 clearPaper 一次性清理
 *
 * 为什么 setMessagesForPaper 支持 updater 函数？
 * - 多个 hooks（useChatSender/useThreadManager/...）依赖 `setMessages(prev => ...)` 模式
 * - 直接传值模式：`setMessagesForPaper(pid, newArray)`
 * - updater 模式：`setMessagesForPaper(pid, prev => newArray)` —— 在并发更新下保证原子性
 */
interface MessagesSlice {
  /** 按 paperId 分桶的消息字典（缺省 paperId 落到 '__default__' 桶） */
  messagesByPaperId: Record<string, ChatMessage[]>;

  /**
   * 设置指定 paperId 的消息列表（支持 value 或 updater）
   * 与 React setState API 形状一致，便于 ChatPanel 做 thin wrapper
   */
  setMessagesForPaper: (
    paperId: string | number | undefined | null,
    msgs: ChatMessage[] | ((prev: ChatMessage[]) => ChatMessage[]),
  ) => void;

  /** 追加单条消息到指定 paperId 的尾部（convenience method） */
  appendMessage: (
    paperId: string | number | undefined | null,
    msg: ChatMessage,
  ) => void;

  /** 清理指定 paperId 的全部 per-paper 状态（messages + 后续 slice 的同 key 字段） */
  clearPaper: (paperId: string | number | undefined | null) => void;
}

/**
 * streamingSlice 状态 + actions（per-paperId）
 *
 * 当前承载：主对话 streaming flag（防止重复发送）。
 * 不含 streamingThreadIds（线程级并行流式），那条还在 useThreadManager 的 Set 里，
 * 后续 PR 再迁过来（需要把 Set 翻成 Record<threadId, boolean> 才能正确触发 selector）。
 *
 * 为什么单独 slice 而不塞到 messagesSlice？
 * - streaming flag 翻转频率远高于 messages 数组变化，分 slice 后 selector 颗粒度更细
 * - 后续 PR 加 streamingThreadIds 时不会污染 messages 订阅者
 */
interface StreamingSlice {
  /** 按 paperId 分桶的主对话 streaming flag */
  streamingByPaperId: Record<string, boolean>;

  /** 设置指定 paperId 的主对话 streaming flag */
  setStreamingForPaper: (
    paperId: string | number | undefined | null,
    streaming: boolean,
  ) => void;
}

export interface ChatState extends SettingsSlice, ModelConfigSlice, MessagesSlice, StreamingSlice {}

export const useChatStore = create<ChatState>()((set, get) => ({
  // ===== settingsSlice =====
  settings: {},
  webSearchEnabled: false,
  threadContextInjection: false,
  tokenBudgets: DEFAULT_TOKEN_BUDGETS,
  customSystemPrompts: {} as Record<AgentType, string>,

  setSettings: (s) => set({ settings: s }),
  setWebSearchEnabled: (b) => set({ webSearchEnabled: b }),
  setThreadContextInjection: (b) => set({ threadContextInjection: b }),
  setTokenBudgets: (b) => set({ tokenBudgets: b }),
  setCustomSystemPrompts: (p) => set({ customSystemPrompts: p }),

  // ===== modelConfigSlice =====
  currentModel: null,
  customConfigs: [],
  selectedConfigId: null,

  setCurrentModel: (m) => set({ currentModel: m }),
  setCustomConfigs: (c) => set({ customConfigs: c }),
  setSelectedConfigId: (id) => set({ selectedConfigId: id }),

  // ===== messagesSlice =====
  messagesByPaperId: {},

  setMessagesForPaper: (paperId, msgs) => {
    const key = paperKey(paperId);
    set((state) => {
      const prev = state.messagesByPaperId[key] ?? [];
      const next = typeof msgs === 'function' ? (msgs as (p: ChatMessage[]) => ChatMessage[])(prev) : msgs;
      return {
        messagesByPaperId: {
          ...state.messagesByPaperId,
          [key]: next,
        },
      };
    });
  },

  appendMessage: (paperId, msg) => {
    const key = paperKey(paperId);
    set((state) => {
      const prev = state.messagesByPaperId[key] ?? [];
      return {
        messagesByPaperId: {
          ...state.messagesByPaperId,
          [key]: [...prev, msg],
        },
      };
    });
  },

  clearPaper: (paperId) => {
    const key = paperKey(paperId);
    set((state) => {
      const nextMessages = { ...state.messagesByPaperId };
      delete nextMessages[key];
      const nextStreaming = { ...state.streamingByPaperId };
      delete nextStreaming[key];
      return {
        messagesByPaperId: nextMessages,
        streamingByPaperId: nextStreaming,
      };
    });
  },

  // ===== streamingSlice =====
  streamingByPaperId: {},

  setStreamingForPaper: (paperId, s) => {
    const key = paperKey(paperId);
    set((state) => ({
      streamingByPaperId: {
        ...state.streamingByPaperId,
        [key]: s,
      },
    }));
  },

  // ===== uiSlice =====
  // (removed) pendingApprovals / toolPanelVisible 已随 Agent 动态工具能力下线移除
}));

/**
 * 从 store 读取指定 paperId 的消息数组（non-reactive）
 *
 * 给非 React 上下文（persistMessages callback、abort 处理等）使用；
 * React 组件应该用 useChatStore selector 订阅。
 */
export function getMessagesForPaper(paperId: string | number | undefined | null): ChatMessage[] {
  return useChatStore.getState().messagesByPaperId[paperKey(paperId)] ?? [];
}

/**
 * 从 store 读取指定 paperId 的 streaming flag（non-reactive）
 *
 * 用于回调里判断「主对话是否正在流式」，避免闭包捕获旧值。
 */
export function getStreamingForPaper(paperId: string | number | undefined | null): boolean {
  return useChatStore.getState().streamingByPaperId[paperKey(paperId)] ?? false;
}

export default useChatStore;
