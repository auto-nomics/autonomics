/**
 * 聊天设置管理 Hook —— Phase 4 store thin wrapper
 *
 * 自 Phase 4 起，状态由 useChatStore（zustand）承载；本 hook 退化为 thin wrapper：
 * - 从 store 订阅状态字段
 * - 保留副作用：API 调用（getSettings/updateSettings/checkTavilyKey）+ antd message
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
import { App } from 'antd';
import { updateSettings } from '../../../services/settingsApi';
import { checkTavilyKey } from '../../../services/searchApi';
import { useChatStore } from '../../../stores/useChatStore';
import type { AgentType } from '../agentTypes';

export function useChatSettings({ paperId, agentType }: {
  paperId?: string | number;
  agentType: AgentType;
}) {
  const { message } = App.useApp();

  // ===== 订阅 store 状态（按字段订阅，避免无关 rerender） =====
  const settings = useChatStore((s) => s.settings);
  const setSettings = useChatStore((s) => s.setSettings);
  const webSearchEnabled = useChatStore((s) => s.webSearchEnabled);
  const setWebSearchEnabled = useChatStore((s) => s.setWebSearchEnabled);
  const threadContextInjection = useChatStore((s) => s.threadContextInjection);
  const setThreadContextInjection = useChatStore((s) => s.setThreadContextInjection);
  const tokenBudgets = useChatStore((s) => s.tokenBudgets);
  const setTokenBudgets = useChatStore((s) => s.setTokenBudgets);
  const customSystemPrompts = useChatStore((s) => s.customSystemPrompts);
  const setCustomSystemPrompts = useChatStore((s) => s.setCustomSystemPrompts);

  // ===== 派生字段（不再独立 fetch —— useChatInit 已经把 settings 加载到 store） =====
  // 旧实现 useEffect 调 getSettings 与 useChatInit 重复, 两个 hook 同时挂载时双请求,
  // client.ts 的 dedup 缓存接住但仍冗余 + 两个 setter 顺序未定可能造成状态竞争。
  // 现在只做 store 内字段的派生解析 (web_search_enabled / thread_context_injection / customSystemPrompts)。
  useEffect(() => {
    const s = (settings ?? {}) as Record<string, unknown> & {
      web_search_enabled?: string;
      thread_context_injection?: string;
      custom_system_prompts?: string;
    };
    setWebSearchEnabled(s?.web_search_enabled === 'true');
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
  }, [settings, setWebSearchEnabled, setThreadContextInjection, setCustomSystemPrompts]);

  // ===== 操作函数（副作用 + store 更新） =====

  /**
   * 处理 Web Search 开关切换
   *
   * 开启前先校验 Tavily Key，校验失败提示用户配置；
   * 校验通过后更新本地状态并持久化到后端。
   */
  const handleWebSearchToggle = useCallback(async (enabled: boolean) => {
    if (enabled) {
      try {
        const result = await checkTavilyKey();
        if (!result.configured) {
          message.warning(result.error || '未配置 Tavily API Key，请在服务器 .env 文件中设置 TAVILY_API_KEY');
          return;
        }
        if (!result.valid) {
          message.error(result.error || 'Tavily API Key 无效，请检查配置');
          return;
        }
      } catch (err) {
        console.warn('Tavily Key 检查请求失败:', err);
      }
    }

    setWebSearchEnabled(enabled);
    try {
      await updateSettings({ web_search_enabled: String(enabled) } as any);
    } catch (err) {
      console.error('保存 Web Search 开关状态失败:', err);
    }
  }, [setWebSearchEnabled, message]);

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
    webSearchEnabled,
    setWebSearchEnabled,
    threadContextInjection,
    setThreadContextInjection,
    tokenBudgets,
    setTokenBudgets,
    handleWebSearchToggle,
    handleThreadContextInjectionToggle,
    customSystemPrompts,
    currentCustomPrompt,
    handleCustomSystemPromptSave,
  };
}
