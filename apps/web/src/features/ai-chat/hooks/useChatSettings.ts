/**
 * 聊天设置管理 Hook —— Phase 4 store thin wrapper
 *
 * 注：jayread 时代的联网搜索开关（webSearchEnabled / handleWebSearchToggle /
 * Tavily Key 校验）已随 searchApi 一并移除 —— autonomics 后端没有联网搜索
 * 能力（plan Phase 5 清扫）。
 *
 * 自 Phase 4 起，状态由 useChatStore（zustand）承载；本 hook 退化为 thin wrapper：
 * - 从 store 订阅状态字段
 * - 保留副作用：API 调用（updateSettings）
 * - 调用方（ChatPanel 等）签名完全不变
 *
 * 为什么要保留这层 wrapper 而不是让 ChatPanel 直接用 store？
 * - 副作用（加载、持久化、Tavily 校验）需要 antd context + 集中错误处理
 * - 现有 4 个面板都通过 useChatSettings 接入，集中修改一处即可
 * - 后续 PR 引入 messagesSlice/streamingSlice 时，调用方迁移成本可逐步分摊
 *
 * @module ai-chat/hooks/useChatSettings
 */

import { useCallback, useEffect } from 'react';
import { updateSettings } from '../../../services/settingsApi';
import { useChatStore } from '../../../stores/useChatStore';
import type { AgentType } from '../agentTypes';

export function useChatSettings({ paperId, agentType }: {
  paperId?: string | number;
  agentType: AgentType;
}) {
  // ===== 订阅 store 状态（按字段订阅，避免无关 rerender） =====
  const settings = useChatStore((s) => s.settings);
  const setSettings = useChatStore((s) => s.setSettings);
  const threadContextInjection = useChatStore((s) => s.threadContextInjection);
  const setThreadContextInjection = useChatStore((s) => s.setThreadContextInjection);
  const tokenBudgets = useChatStore((s) => s.tokenBudgets);
  const setTokenBudgets = useChatStore((s) => s.setTokenBudgets);
  const customSystemPrompts = useChatStore((s) => s.customSystemPrompts);
  const setCustomSystemPrompts = useChatStore((s) => s.setCustomSystemPrompts);

  // ===== 派生字段（不再独立 fetch —— useChatInit 已经把 settings 加载到 store） =====
  // 旧实现 useEffect 调 getSettings 与 useChatInit 重复, 两个 hook 同时挂载时双请求,
  // client.ts 的 dedup 缓存接住但仍冗余 + 两个 setter 顺序未定可能造成状态竞争。
  // 现在只做 store 内字段的派生解析 (thread_context_injection / customSystemPrompts)。
  // web_search_enabled 不再读取：联网搜索已下线。
  useEffect(() => {
    const s = (settings ?? {}) as Record<string, unknown> & {
      thread_context_injection?: string;
      custom_system_prompts?: string;
    };
    setThreadContextInjection(s?.thread_context_injection === 'true');

    // 加载自定义系统提示词（按 agentType 分桶）
    const rawPrompts = s?.custom_system_prompts;
    let parsedPrompts: Record<AgentType, string> = {} as Record<AgentType, string>;
    if (rawPrompts) {
      try {
        const decoded = JSON.parse(rawPrompts);
        if (decoded && typeof decoded === 'object' && !Array.isArray(decoded)) {
          parsedPrompts = decoded as Record<AgentType, string>;
        }
      } catch {
        // JSON 解析失败时丢弃，fallback 空字典
        parsedPrompts = {} as Record<AgentType, string>;
      }
    }
    setCustomSystemPrompts(parsedPrompts);
  }, [settings, setThreadContextInjection, setCustomSystemPrompts]);

  // ===== 操作函数（副作用 + store 更新） =====

  /**
   * 处理线程上下文注入开关切换
   */
  const handleThreadContextInjectionToggle = useCallback(async (enabled: boolean) => {
    setThreadContextInjection(enabled);
    try {
      await updateSettings({ thread_context_injection: String(enabled) } as any);
    } catch (err) {
      console.error('保存线程上下文注入开关状态失败:', err);
    }
  }, [setThreadContextInjection]);

  /**
   * 保存当前 agentType 对应的自定义系统提示词
   * 内部按 agentType 分桶写入 custom_system_prompts（JSON 字符串）
   */
  const handleCustomSystemPromptSave = useCallback(async (value: string) => {
    const trimmed = value || '';
    const next: Record<AgentType, string> = { ...customSystemPrompts, [agentType]: trimmed };
    setCustomSystemPrompts(next);
    try {
      await updateSettings({ custom_system_prompts: JSON.stringify(next) } as any);
    } catch (err) {
      console.error('保存自定义系统提示词失败:', err);
    }
  }, [agentType, customSystemPrompts, setCustomSystemPrompts]);

  /** 当前 agentType 对应的 customSystemPrompt（计算值） */
  const currentCustomPrompt = customSystemPrompts[agentType] || '';

  return {
    settings,
    setSettings,
    threadContextInjection,
    setThreadContextInjection,
    tokenBudgets,
    setTokenBudgets,
    handleThreadContextInjectionToggle,
    customSystemPrompts,
    currentCustomPrompt,
    handleCustomSystemPromptSave,
  };
}
