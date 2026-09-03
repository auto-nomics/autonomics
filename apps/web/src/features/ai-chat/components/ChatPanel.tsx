/**
 * JayRead AI 聊天面板组件
 *
 * 这是论文阅读器的核心交互组件，负责：
 * 1. 与 AI 进行流式对话（支持 OpenAI 和 Anthropic 两种 API）
 * 2. 消息持久化（通过 REST API 保存/加载历史）
 * 3. 接收外部上下文注入（PDF 选中文本、段落导览等）
 * 4. 使用 forwardRef + useImperativeHandle 暴露方法给父组件
 *
 * 为什么使用 forwardRef？
 * - 父组件（PaperReaderPage）需要主动调用 ChatPanel 的 injectContext 方法
 * - forwardRef 允许将 ref 转发给函数组件，使其可以暴露命令式 API
 * - 这种模式在"父组件需要触发子组件行为"的场景中很常见
 *
 * 为什么 AI 调用使用浏览器直连而非后端代理？
 * - OpenAI 和 Anthropic API 均支持 CORS，浏览器可直接调用
 * - 减少后端复杂度，避免大文本在前后端间传输
 * - 用户的 API Key 仅在本地使用，不经过服务器，更安全
 * - 流式响应（SSE）在浏览器端处理更自然
 *
 * 代码拆分说明：
 * - useChatSender：发送消息核心逻辑（系统提示词构建、API 请求、SSE 流式响应解析）
 * - useSlashCommands：斜杠命令处理（/new, /resume, /prompt, /attachment, /model, /clear, /hide）
 * - useConversationManager：会话管理（加载列表、恢复历史、删除会话）
 * - useModelConfig：模型配置管理（模型选择、自定义配置、配置持久化）
 *
 * @module ai-chat/ChatPanel
 */

// ===== React 核心钩子导入 =====
import React, { useState, useEffect, useCallback, useRef, forwardRef, useImperativeHandle, useMemo } from 'react'; // 导入 React 核心钩子：状态、副作用、回调缓存、引用、转发 ref、命令式 API、备忘录

// ===== Ant Design 组件导入 =====
import { Spin, Empty, Button, App, Dropdown } from 'antd'; // 导入 Ant Design 组件：加载动画(Spin)、空状态占位(Empty)、按钮(Button)、全局消息/弹窗(App)、下拉菜单(Dropdown)

// ===== Ant Design 图标导入 =====
import { CloseOutlined, MessageOutlined } from '@ant-design/icons'; // CloseOutlined: 关闭/删除图标（用于引用消息取消、追问指示条关闭等） MessageOutlined: 消息图标（用于追问指示条、转为追问按钮）

// ===== 子组件导入 =====
import NiceModal from '@ebay/nice-modal-react';

// ===== API 层导入 =====
import { saveMessages } from '../../../services/chatApi'; // saveMessages：保存消息到后端（getMessages 已移入 useChatInit hook）
import request from '../../../services/client'; // 导入 HTTP 请求客户端（基于 fetch 的封装，用于调用后端 REST API）
import { getSettings, updateSettings } from '../../../services/settingsApi'; // 导入设置 API：getSettings 获取用户配置，updateSettings 更新用户配置
import { useChatStore, getMessagesForPaper, getStreamingForPaper } from '../../../stores/useChatStore'; // Phase 4: 消息 + streaming 状态外移到 zustand store（per-paperId 分桶）

/**
 * 稳定的空数组引用 —— 用于 zustand selector 的 fallback。
 *
 * 为什么不能用 `?? []`？
 * - zustand 用 Object.is 比较 selector 返回值决定是否 rerender
 * - 每次渲染 `?? []` 会创建新的数组实例 → Object.is 失败 → 触发 rerender → 再读 selector → 又新数组 → 死循环
 * - Maximum update depth exceeded 就是这么来的
 *
 * 必须用 module-level 常量，让 fallback 始终返回同一引用。
 */
const EMPTY_ARRAY: readonly never[] = Object.freeze([]) as readonly never[];

// ===== UI 子组件导入 =====
import ChatMessageItem from './ChatMessageItem'; // 导入单条消息渲染组件：负责渲染单条聊天气泡（用户/AI），包含 Markdown 渲染、代码高亮、操作菜单等
import SlateInputWithSender from './SlateInputWithSender'; // 富文本输入框组件：基于 Slate.js 的富文本编辑器，支持斜杠命令提示、附件上传、发送按钮等

// ===== 样式导入 =====
import '../styles.css'; // 导入 AI Chat 面板样式（包含 Markdown 样式和主题变量）

// ===== 工具函数导入 =====
import { formatCustomModelLabel, zoteroL10n } from '../zoteroL10n'; // formatCustomModelLabel: 格式化自定义模型标签名称 zoteroL10n: 国际化翻译函数

// ===== 常量导入 =====
import { DEFAULT_TOKEN_BUDGETS } from '../utils/chatConstants'; // 导入 token 预算默认配置常量（各模型的上下文窗口限制等）
import type { AgentType } from '../agentTypes';
import { DEFAULT_PERSONAS, getDefaultPersonaSummary } from '../agentTypes';

// ===== 核心 Hooks 导入 =====
import { useChatSender } from '../hooks/useChatSender'; // 导入 AI 聊天发送 hook（封装发送消息的核心逻辑：系统提示词构建、API 请求、SSE 流式响应解析）
import { useSlashCommands } from '../hooks/useSlashCommands'; // 导入斜杠命令处理 hook（处理 /new, /resume, /prompt, /attachment, /model, /clear, /hide）
import { useConversationManager } from '../hooks/useConversationManager'; // 导入会话管理 hook（加载列表、恢复历史、删除会话）
import { useModelConfig } from '../hooks/useModelConfig'; // 导入模型配置管理 hook（模型选择、自定义配置、配置持久化）

// ===== Phase 5/6: 追问线程相关导入 =====
import { useThreadManager } from '../hooks/useThreadManager'; // Phase 5: 导入线程状态管理 hook（处理线程创建、删除、折叠、发送等操作）
import { MAX_THREAD_MESSAGES, MAX_SUBTHREAD_MESSAGES } from '../utils/threadUtils'; // 追问线程最大消息数量常量（ensureThreadStructure 已移入 useChatInit hook）

// ===== 搜索与导览相关导入 =====
import { useChatSearch } from '../hooks/useChatSearch'; // 导入聊天搜索 hook（处理搜索、过滤、高亮）
import { useGuideData } from '../hooks/useGuideData'; // 导入导览数据 hook（加载段落导览、章节语义、图片上下文）
import { useChatSettings } from '../hooks/useChatSettings'; // 导入聊天设置 hook（加载用户设置、开关状态、token 预算）
import { useTemplateManager } from '../hooks/useTemplateManager'; // 导入模板管理 hook（模板数据加载、保存、弹窗状态）
import { useActiveThread } from '../hooks/useActiveThread'; // 导入追问线程激活状态管理 hook（管理当前激活的追问线程/子线程信息）
import { useEmbeddingStatus } from '../hooks/chat-side-effects/useEmbeddingStatus'; // 嵌入状态轮询 hook
import { useSmartScroll } from '../hooks/chat-side-effects/useSmartScroll'; // 智能滚动 hook
import { useChatInit } from '../hooks/chat-side-effects/useChatInit'; // 聊天初始化 hook
import { useMessageActions } from '../hooks/chat-side-effects/useMessageActions'; // 消息级动作 hook

// ===== 会话恢复相关导入 =====
// (removed) useSessionRecovery / RecoveryBanner 已随多 Agent team 模式移除

/**
 * ChatPanel 组件
 *
 * 使用 forwardRef 包裹，允许父组件通过 ref 调用 injectContext 方法
 *
 * @param {object} props - 组件属性
 * @param {number} props.paperId - 当前论文 ID，用于关联聊天记录
 * @param {object|null} props.injectedContext - 从外部注入的上下文数据
 *   当用户在 PDF 中选中文本或点击段落导览的"深入提问"时，此值会被更新
 *   包含 type（来源类型）、payload（上下文数据）、prompt（预设提示词）
 * @param {string} [props.markdown] - 论文的完整 Markdown 内容（可选）
 *   用于构建分层上下文注入，按优先级和 token 预算决定注入量
 * @param {object} [props.paperInfo] - 论文基础信息（可选）
 *   包含 title、authors、abstract 等元数据，用于 Layer 1 上下文
 * @param {object} [props.paper] - 论文完整信息（可选）
 *   包含 parse_status（解析状态）等字段，用于检查论文是否已解析完成
 *   只有 parse_status === 'done' 时才会在 Layer 4 注入完整 Markdown
 * @param {boolean} [props.collapsed] - 面板折叠状态，折叠时不渲染内容
 * @param {function} [props.onToggleCollapse] - 折叠/展开切换回调
 * @param {string} [props.chatSearchKeyword] - 统一搜索栏的聊天搜索关键词（由 PaperReaderPage 中继传递）
 * @param {function} [props.onChatSearchClear] - 搜索结果点击后清空搜索关键词的回调
 * @param {function} [props.onNavigateToPapers] - /paper 斜杠命令导航回调（由 ChatHomePage 提供）
 * @param {React.Ref} ref - 转发的 ref，用于暴露命令式 API（sendMessage、injectContext）
 */
const ChatPanel = forwardRef(function ChatPanel({
  agentType,
  paperId,
  markdown,
  paperInfo,
  paper,
  collapsed,
  onToggleCollapse,
  chatSearchKeyword = '',
  onChatSearchClear,
  onNavigateToPapers,
}: { agentType: AgentType } & any, ref) {
  // paper prop：包含论文完整信息，包括 parse_status（解析状态）等字段

  // ===== Ant Design 全局消息 API =====
  // 通过 App.useApp() 获取 message 实例，用于显示全局提示（success、error、warning 等）
  // 注意：必须包裹在 <App> 组件内才能使用
  const { message } = App.useApp();

  // ========================================================================
  // ===== 状态管理（核心状态） =====
  // ========================================================================

  // Phase 4: messages 状态外移到 useChatStore.messagesByPaperId（per-paperId 分桶）
  // - store 是单一数据源，子 hook 仍通过 setMessages prop 操作（API 不变）
  // - 不再需要 messagesRef + sync effect：getState() 天然读取最新值
  const messages = useChatStore((s) => s.messagesByPaperId[String(paperId ?? '__default__')] ?? EMPTY_ARRAY) as any[];
  const setMessages = useCallback<React.Dispatch<React.SetStateAction<any[]>>>((value) => {
    useChatStore.getState().setMessagesForPaper(
      paperId,
      typeof value === 'function' ? (value as (prev: any[]) => any[]) : value,
    );
  }, [paperId]);
  // loading 已移入 useChatInit hook（初始加载状态由 hook 管理）
  // Phase 4: streaming 状态外移到 useChatStore.streamingByPaperId（per-paperId 分桶）
  // - 不再需要 streamingRef + sync effect：getStreamingForPaper(paperId) 天然读取最新值
  const streaming = useChatStore((s) => s.streamingByPaperId[String(paperId ?? '__default__')] ?? false);
  const setStreaming = useCallback((s: boolean) => {
    useChatStore.getState().setStreamingForPaper(paperId, s);
  }, [paperId]);
  const [webSearching, setWebSearching] = useState(false); // 网络搜索是否进行中（用于显示搜索指示器）
  const [systemPrompt, setSystemPrompt] = useState(''); // 系统提示词（包含论文上下文）
  // embeddingStatus / embedTriggeredRef 已抽出到 useEmbeddingStatus hook

  // ===== 追问线程激活状态（已移至 useActiveThread hook）=====
  // activeThread 状态和激活/取消/创建回调由 hook 管理

  // ===== 网络搜索来源指示器相关状态 =====
  // 存储最近一次网络搜索的来源列表（用于在 AI 回复底部显示搜索来源引用）
  // 数据结构：[{ title: string, url: string }] 或 null（表示最近一次回复未使用网络搜索）
  // 每次新的 handleSend 调用时重置，仅在 webSearchEnabled 且搜索成功时赋值
  const [lastWebSearchSources, setLastWebSearchSources] = useState<any>(null);

  // ========================================================================
  // ===== 多 Agent 团队模式状态 =====
  // ========================================================================

  // (removed) teamPanelVisible / pendingDelegateApprovals state 已随多 Agent team 模式移除

  // ========================================================================
  // ===== Ref 定义（需要在 hooks 之前定义，因为 hooks 依赖这些 ref） =====
  // ========================================================================

  // SlateInputWithSender 的 ref，用于命令式操作（如插入文本、打开配置弹窗）
  const slateInputRef = useRef<any>(null);

  // 用于管理流式 fetch 请求的 AbortController 引用
  // 当用户切换论文（paperId 变化）时，需要中断上一个未完成的 AI 流式请求
  // 避免旧请求的响应内容写入新论文的消息状态中
  const abortControllerRef = useRef<any>(null);

  // 用于跟踪当前活跃的流式请求 ID
  // 每次发送新消息时生成新的唯一 ID，用于在流式响应到达时验证是否为当前活跃请求
  // 防止用户快速切换论文时，旧的流式响应覆盖新论文的消息状态
  const activeRequestIdRef = useRef<any>(null);

  // Phase 4: messagesRef + streamingRef + sync effect 已全部删除
  // - persistMessages 通过 getMessagesForPaper(paperId) 在回调里读最新值
  // - useActiveThread 改用 getMessages getter（不再依赖 ref）
  // - 防重复发送检查用 getStreamingForPaper(paperId) 替代 streamingRef.current

  // 消息列表底部元素的 ref，用于自动滚动到底部
  const messagesEndRef = useRef<any>(null); // ref：指向消息列表底部的空 div 元素

  // 消息列表可滚动容器 ref，用于搜索点击跳转时 scrollIntoView
  const messageListRef = useRef<any>(null); // ref：消息列表可滚动容器

  // isNearBottomRef 已移入 useSmartScroll hook（智能滚动状态由 hook 管理）

  // ========================================================================
  // ========== 使用提取的 Hooks（按依赖顺序排列）==========
  // ========================================================================

  // ----- 1. 聊天设置 hook -----
  // 加载并管理用户偏好设置，包括：语义搜索开关、网络搜索开关、线程上下文注入开关、
  // token 预算配置、自定义系统提示词等
  const {
    settings,                   // 用户设置对象（从后端加载，包含 API Key、模型偏好等）
    setSettings,                // 更新用户设置对象的函数
    webSearchEnabled,           // 网络搜索是否开启（发送消息时自动搜索互联网）
    threadContextInjection,     // 线程上下文注入是否开启（将追问线程历史注入系统提示词）
    tokenBudgets,               // Token 预算配置（各模型的上下文窗口限制）
    handleWebSearchToggle,      // 切换网络搜索开关的函数
    handleThreadContextInjectionToggle, // 切换线程上下文注入开关的函数
    currentCustomPrompt: customSystemPrompt, // 当前 agentType 对应的自定义系统提示词（覆盖该 agent 的默认 persona）
    handleCustomSystemPromptSave, // 保存自定义系统提示词的函数（内部按 agentType 分桶写入）
  } = useChatSettings({
    paperId,                    // 论文 ID，用于加载该论文的专属设置
    agentType,                  // 当前面板的 agent 类型（决定 persona 字典取哪一份）
  });

  // ----- 2. 导览数据 hook -----
  // 加载论文的段落导览、章节语义元数据、图片系统上下文等数据
  // 这些数据用于构建分层系统提示词（Layer 3: 段落导览）
  const {
    guideSections,              // 段落导览数据：[{ title, paragraphs: [{ text, summary }] }] 结构的章节/段落数组
    sectionSemantics,           // 章节语义元数据：用于 Layer 4 相关性增强匹配
    imageSystemContext,         // 图片系统上下文：论文中图片的描述和说明，用于构建视觉相关的系统提示词
    transformV2TreeToV1Sections, // 将 V2 版本的树形导览数据转换为 V1 版本的扁平数组（兼容性处理）
  } = useGuideData({
    paperId,                    // 论文 ID，用于加载该论文的导览数据
    paper,                      // 论文完整对象，包含解析状态等信息
  });

  // ========== 使用提取的 Hooks ==========

  // ----- 3. 斜杠命令处理 hook -----
  // 处理用户在输入框中输入的斜杠命令（如 /new, /resume, /prompt 等）
  const {
    handleSlashCommand,        // 处理斜杠命令的主函数：接收输入文本，如果匹配斜杠命令则执行并返回 true
    handleNewConversation,      // 创建新对话的函数：清空当前消息列表，开始全新的对话
    attachmentInputRef,         // 附件文件选择 input 的 ref：用于 /attachment 命令触发文件选择器
    handleAttachmentFileChange, // 附件文件选择处理函数：用户选择文件后，将文件作为附件添加到输入框
    slashCommandList,           // 斜杠命令列表（用于输入框的自动补全提示展示）
  } = useSlashCommands({
    paperId,             // 论文 ID
    persistMessages: (msgs: any) => saveMessages(paperId, msgs), // 持久化消息的函数（包装 saveMessages）
    onToggleCollapse,    // 收起/展开对话面板的回调（/hide 命令使用）
    slateInputRef,       // SlateInputWithSender 组件的 ref（用于 /attachment 命令插入附件）
    setMessages,         // 设置消息列表的函数（/new 命令清空消息时使用）
    onNavigateToPapers,  // /paper 命令导航回调
  });

  // ----- 4. 会话管理 hook -----
  // 管理对话会话的生命周期：历史会话列表加载、恢复、删除
  const {
    conversations,             // 对话会话列表：[{ id, title, messages, createdAt }] 数组
    loadConversations,         // 加载对话列表：从后端获取当前论文的所有历史对话
    handleResumeConversation,  // 恢复指定的历史对话：将选中的历史对话消息加载到当前消息列表
    handleDeleteConversation,  // 删除指定的对话会话：从后端删除并从列表中移除
  } = useConversationManager({
    paperId,      // 论文 ID
    setMessages,  // 设置消息列表的函数
  });

  // ----- 5. 模型配置管理 hook -----
  // 管理 AI 模型选择和自定义模型配置（支持 OpenAI、Anthropic 等多种模型提供商）
  const {
    currentModel,              // 当前选中的模型对象：{ key, label, config?: { provider, modelName, ... } }
    customConfigs,             // 自定义模型配置列表：[{ id, name, modelName, provider, ... }] 数组
    selectedConfigId,          // 当前选中的自定义配置 ID（字符串或 null）
    setCurrentModel,           // 设置当前模型的函数
    initializeCustomConfigs,   // 初始化自定义模型配置：从用户设置中加载已保存的配置
    handleModelChange,         // 处理模型切换：更新当前模型并持久化到设置
    handleSaveConfigs,         // 保存自定义模型配置列表：序列化到后端 settings API
    handleConfigIdChange,      // 处理配置 ID 切换：切换到指定的自定义配置
    displayModel,              // 当前模型的显示名称（经过格式化，用于 UI 展示）
    dropdownModelItems,        // 模型下拉菜单项：用于头部模型选择器的菜单数据
    selectedMenuKey,           // 当前选中的菜单项 key（用于 Dropdown 组件的 selectedKeys）
    menuSelectedKeys,          // 菜单选中的 keys 集合（数组，用于 Dropdown 组件高亮当前模型）
  } = useModelConfig({
    settings,    // 用户设置对象（从后端加载，包含模型偏好等配置）
  });

  /**
   * 持久化消息到后端
   *
   * 使用 useCallback 缓存函数引用，避免每次渲染创建新函数
   * 只有 paperId 变化时才重新创建（切换论文时）
   *
   * 此函数还负责清理线程消息数据：
   * - 递归处理所有线程和子线程中的消息
   * - 确保每条 ThreadMessage 都有唯一 id 字段
   * - 兼容旧数据中缺少 id 的情况（自动生成时间戳+随机数的 id）
   */
  const persistMessages = useCallback(async (msgs?: any) => { // useCallback 缓存持久化函数
    // 无参数时从 store 读取最新状态，确保线程等动态数据不丢失
    const messagesToSave = msgs || getMessagesForPaper(paperId);
    try { // 开始 try-catch
      // 清理线程消息：确保所有 ThreadMessage 都有 id 字段
      // 兼容旧数据中 parseStreamResponse 创建的缺少 id 的 assistant 消息
      // 递归处理子线程：m.threads[].messages[].threads[].messages[]
      const sanitizeThreadMessages = (threadMessages: any[]) =>
        threadMessages.map((tm: any) => {
          // 如果消息已有 id 则保留，否则生成一个基于时间戳+随机数的唯一 id
          const base = tm.id ? tm : { ...tm, id: Date.now().toString(36) + Math.random().toString(36).slice(2) };
          // 递归处理子线程中的消息
          if (!base.threads?.length) return base;
          return {
            ...base,
            threads: base.threads.map((st: any) => ({
              ...st,
              messages: sanitizeThreadMessages(st.messages || []),
            })),
          };
        });

      // 对主消息列表中包含线程的消息进行清理
      const sanitized = messagesToSave.map((m: any) => {
        if (!m.threads || m.threads.length === 0) return m; // 无线程的消息直接返回
        return {
          ...m,
          threads: m.threads.map((t: any) => ({
            ...t,
            messages: sanitizeThreadMessages(t.messages || []),
          })),
        };
      });
      await saveMessages(paperId, sanitized); // 调用 API 保存清理后的消息
    } catch (err) { // 捕获保存错误
      // 静默失败，不打扰用户（聊天功能不依赖持久化）
      console.error('保存聊天记录失败:', err); // 控制台输出错误
    }
  }, [paperId]); // 依赖 paperId

  // ========================================================================
  // 将本地加载的图片上下文整合为 additionalSystemContext 传给 useChatSender
  const mergedAdditionalContext = useMemo(() => imageSystemContext, [imageSystemContext]);

  // ========================================================================
  // ===== 上下文预览数据（只读，用于 TemplateManagerModal 的"上下文预览"标签页）=====
  // ========================================================================
  // 该数据展示了当前系统提示词的分层注入结构，帮助用户了解哪些上下文会被注入到 AI 对话中
  // 使用 useMemo 缓存计算结果，避免每次渲染都重新构建
  const contextPreviewData = useMemo(() => {
    const layers = []; // 上下文层列表

    // Layer 1: 论文基础信息
    // 包含论文标题、作者、摘要等元数据，作为系统提示词的第一层上下文
    if (paperInfo) {
      layers.push({
        key: 'L1',
        label: '论文基础信息',
        active: true, // 有论文信息时标记为活跃
        details: [
          paperInfo.title ? `标题: ${paperInfo.title}` : null,
          paperInfo.authors
            ? `作者: ${Array.isArray(paperInfo.authors) ? paperInfo.authors.join(', ') : paperInfo.authors}`
            : null,
          paperInfo.abstract ? '摘要: (已加载)' : null,
        ].filter(Boolean), // 过滤掉 null 值
      });
    } else {
      layers.push({
        key: 'L1',
        label: '论文基础信息',
        active: false, // 无论文信息时标记为未激活
        details: [`无论文信息（使用 ${getDefaultPersonaSummary(agentType)} 默认 persona）`],
      });
    }

    // Layer 1.5: 线程上下文注入
    // 统计当前消息中已有的追问线程数量，判断线程上下文注入是否生效
    const threadCount = messages.reduce((count: number, m: any) => count + (m.threads?.length || 0), 0);
    layers.push({
      key: 'L1.5',
      label: '线程上下文注入',
      active: threadContextInjection && threadCount > 0, // 开关打开且有线程数据时才生效
      details: [
        threadContextInjection
          ? (threadCount > 0 ? `${threadCount} 个线程` : '已开启，暂无线程')
          : '未开启',
      ],
    });

    // Layer 2.5: 网络搜索
    layers.push({
      key: 'L2.5',
      label: '网络搜索',
      active: webSearchEnabled,
      details: [webSearchEnabled ? '已开启（发送时自动搜索）' : '未开启'],
    });

    // Layer 2.7: 高优先级上下文
    // 额外系统上下文中优先级为 'high' 的条目（如图片描述等）
    const highPriority = (mergedAdditionalContext || []).filter((c: any) => c.priority === 'high');
    layers.push({
      key: 'L2.7',
      label: '高优先级上下文',
      active: highPriority.length > 0,
      details: [highPriority.length > 0 ? `${highPriority.length} 条注入` : '无'],
    });

    // Layer 3: 段落导览
    // 论文章节和段落的摘要信息，帮助 AI 理解论文结构
    const guideSectionCount = guideSections?.length || 0; // 章节数量
    const guideParaCount = guideSections?.reduce((sum: number, s: any) => sum + (s.paragraphs?.length || 0), 0) || 0; // 段落总数
    layers.push({
      key: 'L3',
      label: '段落导览',
      active: guideSectionCount > 0,
      details: [guideSectionCount > 0
        ? `${guideSectionCount} 个章节，${guideParaCount} 个段落`
        : '无导览数据'],
    });

    // Layer 4: RAG 语义检索
    // 基于用户查询自动检索相关论文片段，需要论文已完成解析和嵌入
    const isParseDone = paper?.parse_status === 'done';
    layers.push({
      key: 'L4',
      label: 'RAG 语义检索',
      active: !!paperId && isParseDone,
      details: [isParseDone
        ? '已解析，发送时自动检索相关片段'
        : (paper?.parse_status ? `解析状态: ${paper.parse_status}` : '未解析')],
    });

    // Layer 5: 低优先级上下文
    // 额外系统上下文中优先级非 'high' 的条目
    const lowPriority = (mergedAdditionalContext || []).filter((c: any) => c.priority !== 'high');
    layers.push({
      key: 'L5',
      label: '低优先级上下文',
      active: lowPriority.length > 0,
      details: [lowPriority.length > 0 ? `${lowPriority.length} 条注入` : '无'],
    });

    return layers; // 返回构建好的上下文层列表
  }, [paperInfo, threadContextInjection, messages, webSearchEnabled, mergedAdditionalContext, guideSections, paperId, paper?.parse_status]);

  // ========================================================================
  // ===== 调用 useChatSender hook 获取 sendMessages 函数 =====
  // ========================================================================
  // hook 内部封装了系统提示词构建、API 请求发送、SSE 流式响应解析等完整逻辑
  // 传入所有必要的状态和回调函数作为参数，hook 返回可直接调用的 sendMessages 方法
  const { sendMessages } = useChatSender({ // 解构获取 sendMessages 发送函数
    setMessages,               // 设置消息列表的函数（用于 hook 内部更新消息状态）
    setStreaming,               // 设置流式状态的函数（用于 hook 内部控制发送中状态）
    setWebSearching,            // 设置搜索状态的函数（用于 hook 内部控制网络搜索 UI）
    setLastWebSearchSources,    // 设置搜索来源的函数（用于 hook 内部保存搜索结果来源）
    persistMessages,            // 持久化消息的函数（用于 hook 内部保存对话到后端）
    abortControllerRef,         // AbortController 引用（用于 hook 内部管理请求中断）
    activeRequestIdRef,         // 活跃请求 ID 引用（用于防止旧流式响应覆盖新消息）
    settings,                   // 用户设置对象（包含 API Key、模型配置等）
    currentModel,               // 当前选中的模型对象（包含模型名称、自定义配置等）
    tokenBudgets,               // Token 预算配置（各模型的 token 限制）
    paperInfo,                  // 论文基础信息（标题、作者、摘要，用于 Layer 1）
    paper,                      // 论文完整信息（包含 parse_status，用于判断是否注入全文）
    guideSections,              // 段落导览数据（用于 Layer 3 段落摘要注入）
    messages,                   // 当前消息列表（用于 hook 内部构建完整消息历史）
    streaming,                  // 是否正在流式回复中（用于 hook 内部防重复发送校验）
    additionalSystemContext: mergedAdditionalContext as any, // 合并后的额外系统上下文（含图片信息）
    threadContextInjection,     // Phase 6: 线程上下文注入开关状态
    onError: (errorMsg) => {
      const msg = errorMsg || '请求失败';
      message.error(msg);
      // 同时在聊天里添加持久错误消息(不只是 toast)
      setMessages((prev: any[]) => [
        ...prev,
        {
          role: 'assistant',
          content: `❌ **${msg}**`,
          id: `msg-error-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
        },
      ]);
    },
    customSystemPrompt,         // 用户自定义系统提示词（覆盖默认人设）
    agentType,                  // 当前面板的 agent 类型（systemPromptBuilder 据此选 persona）
    tools: [], // 工具挂载由后端 persona 决定（web_search / smart_fetch / pubmed）
  });

  // ========================================================================
  // ========== Phase 5: 线程状态管理 ==========
  // ========================================================================
  // useThreadManager 抽取了线程相关的状态管理逻辑，包括：
  // - 线程的创建、删除、折叠/展开操作
  // - 线程内消息发送（复用 useChatSender 的 sendMessages）
  // - 线程持久化（debounce 2s + AI 回复完成时立即保存）
  // - 流式状态管理（streamingThreadId）
  const {
    isMainStreaming,         // 主对话是否正在流式回复中
    isThreadStreaming,       // 是否有线程正在流式回复中
    handleCreateThread,      // 创建新线程
    handleDeleteThread,      // 删除线程
    handleToggleThreadCollapse, // 切换线程折叠状态
    handleCollapseAll,       // 折叠所有线程
    handleExpandAll,         // 展开所有线程
    handleExpandThreadById,  // Phase 6: 展开指定线程（用于搜索时自动展开匹配的线程）
    handleThreadSendMessage, // 发送追问消息
    isThreadCollapsed,       // 判断线程是否折叠
    handleDeleteThreadMessage, // Phase 6: 删除线程内单条消息
    handleEditThreadMessage,   // Phase 6: 编辑线程内单条消息
    // 子线程方法（Phase 6 新增）
    handleCreateSubThread,       // 创建子线程（二级追问）
    handleSubThreadSendMessage,  // 子线程发送消息
    handleDeleteSubThread,       // 删除子线程
    handleToggleSubThreadCollapse, // 切换子线程折叠状态
    handleDeleteSubThreadMessage, // 删除子线程内单条消息
    handleEditSubThreadMessage,   // 编辑子线程内单条消息
  } = useThreadManager({
    messages,          // 当前消息列表
    setMessages,        // 设置消息列表的函数
    streaming,          // 主对话流式状态
    sendMessages: sendMessages as any,       // 发送消息的函数（useChatSender 提供）
    persistMessages,    // 持久化消息的函数
    paperInfo,          // 论文基础信息（用于线程系统提示词）
  });

  // ========================================================================
  // ========== 追问线程激活状态管理（useActiveThread hook）==========
  // ========================================================================
  // 管理当前激活的追问线程/子线程信息，控制输入框的追问模式切换
  const {
    activeThread,                // 当前激活的线程信息：{ msgIdx, threadId, subThreadId? } 或 null
    activeThreadInfo,            // 激活线程的详细信息：{ anchorText, messageCount, isSubThread, isAtLimit } 或 null
    handleActivateThread,        // 激活指定的一级线程
    handleDeactivateThread,      // 取消激活（退出追问模式），恢复普通对话模式
    handleCreateAndActivateThread, // 创建新线程并自动激活（一步操作）
    handleActivateSubThread,     // 激活指定的子线程
    handleCreateAndActivateSubThread, // 创建新子线程并自动激活
  } = useActiveThread({
    messages,                    // 当前消息列表
    getMessages: useCallback(() => getMessagesForPaper(paperId) as any, [paperId]), // Phase 4: 替代 messagesRef，从 store 读最新值
    isThreadCollapsed,           // 判断线程是否折叠的函数
    handleToggleThreadCollapse,  // 切换线程折叠状态的函数
    handleCreateThread,          // 创建线程的函数
    handleCreateSubThread: handleCreateSubThread as any,       // 创建子线程的函数
    handleToggleSubThreadCollapse, // 切换子线程折叠状态的函数
    slateInputRef,               // 输入框 ref（用于激活线程后自动聚焦输入框）
  });

  // ========================================================================
  // ----- 聊天搜索 hook -----
  // ========================================================================
  // 处理聊天消息的搜索、过滤和关键词高亮
  // 注意：必须在 useThreadManager 之后调用，因为依赖 handleExpandThreadById
  const {
    filteredMessages,            // 根据搜索关键词过滤后的消息列表
    highlightChatKeyword,        // 在文本中高亮搜索关键词的函数
    messageIndexMap,             // 消息对象到原始索引的映射表（Map，O(1) 查找）
    handleSearchResultClick,     // 搜索结果点击处理函数：清空搜索并跳转到目标消息
  } = useChatSearch({
    messages,                    // 完整的消息列表
    chatSearchKeyword,           // 当前搜索关键词
    onChatSearchClear,           // 清空搜索关键词的回调
    handleExpandThreadById,      // 展开指定线程的函数（搜索命中线程内容时自动展开）
  });

  // ========================================================================
  // ========== 聊天搜索：消息过滤和关键词高亮 ==========
  // ========================================================================

  /**
   * 根据搜索关键词过滤聊天消息（Phase 6: 支持线程内容搜索）
   *
   * 使用 useMemo 缓存计算结果，避免每次渲染都重新过滤：
   * - 无搜索关键词时直接返回原始消息列表（零开销）
   * - 有搜索关键词时，使用 messageMatchesSearch 同时搜索主消息和线程内容
   * - 支持纯文本和 multimodal（数组格式）的消息内容
   * - 线程消息也会被搜索，命中时自动展开
   *
   * Phase 6 改进：
   * - 原实现只检查主消息 content
   * - 新实现使用 messageMatchesSearch 同时检查 threads[].messages[].content
   * - 搜索命中线程时，该线程会自动展开（见上方 useEffect）
   *
   * 为什么用 useMemo 而不是 useState + useEffect？
   * - filteredMessages 完全由 messages 和 chatSearchKeyword 派生，不需要额外的状态管理
   * - useMemo 只在依赖变化时重新计算，比 useEffect + setState 更高效
   * - 避免了 useEffect 中的"渲染 -> 计算更新状态 -> 再渲染"双重渲染问题
   */

  /**
   * 在文本中高亮搜索关键词
   *
   * 将匹配关键词的文本片段包裹在 <mark className="chat-search-highlight"> 元素中。
   * 使用不区分大小写的正则匹配，但保留原始大小写（高亮显示原始文本）。
   *
   * 为什么用正则 split 而不是手动 indexOf 循环？
   * - 正则 split 一次调用即可将文本拆分为匹配和非匹配的交替片段
   * - 代码更简洁，不易出错
   * - 性能对于聊天消息长度来说完全足够
   *
   * @param {string} text - 需要高亮处理的文本内容
   * @returns {Array<string|JSX.Element>} 混合了纯文本和高亮元素的数组，作为 React 子元素渲染
   */

  /**
   * 处理搜索结果消息点击：清空搜索并跳转到该消息在完整列表中的位置
   *
   * 交互流程：
   * 1. 用户在搜索模式下点击某条匹配消息
   * 2. 通过 onChatSearchClear 回调清空搜索关键词（恢复显示全部消息）
   * 3. 使用 requestAnimationFrame 等待 React 重渲染完成后滚动到目标消息
   *
   * 为什么使用 requestAnimationFrame？
   * - 清空搜索关键词后 React 需要重渲染（从 filteredMessages 切换到完整 messages）
   * - DOM 更新是异步的，直接 scrollIntoView 会找不到目标元素
   * - requestAnimationFrame 确保在下一帧（重渲染完成之后）执行滚动
   *
   * @param {number} originalIdx - 消息在原始 messages 数组中的索引
   */

  /**
   * 构建消息索引映射表（O(n^2) -> O(n) 优化）
   *
   * 为什么需要这个映射表？
   * - filteredMessages 是过滤后的消息子集，需要找到每条消息在原始 messages 数组中的索引
   * - 原先的实现在 .map() 内部使用 messages.indexOf(msg)，每次查找都是 O(n)
   * - 使用 Map 缓存消息 -> 索引的映射，将查找复杂度降为 O(1)
   *
   * 使用消息对象的引用作为 key（WeakMap 的特性），避免内存泄漏
   * 由于 Map 使用引用相等性（===）比较 key，消息对象的引用可以直接作为 key
   */

  // ========================================================================
  // ===== 其他 UI 状态 =====
  // ========================================================================

  // 模型选择器显示状态：通过右键输入区域切换显示
  const [showModelSelector, setShowModelSelector] = useState(false);

  // 引用消息状态：当用户右键某条消息选择"引用"时设置
  // 数据结构：{ role: string, content: string, originalIdx: number } 或 null
  const [quotedMessage, setQuotedMessage] = useState<any>(null);

  // ========================================================================
  // ----- 模板管理 hook -----
  // ========================================================================
  // 管理自定义 prompt 模板的数据加载、保存和弹窗状态
  const {
    templateData,               // 当前模板数据：{ templates: [...] }
    loadTemplates,              // 加载模板数据的函数（从 settings 中解析）
    handleSaveTemplates,        // 保存模板数据的函数（序列化到 settings API + 注册到 ContextBridge）
  } = useTemplateManager();

  // ===== 状态定义（已提取到 hooks 的状态不再在此定义）=====

  /**
   * 初始化（已抽出到 useChatInit hook）
   *
   * paperId 变化时：中断上一个流式请求 → 并行加载聊天历史 + 用户设置
   * （桌面端启动时 sidecar 可能还在编译，getSettings 自动重试最多 15 秒）
   */
  const { loading } = useChatInit({
    paperId,
    setMessages,
    setSettings,
    setStreaming,
    initializeCustomConfigs,
    loadTemplates,
    abortControllerRef,
    activeRequestIdRef,
    onMessagesLoadError: (errMsg) => message.error('加载聊天记录失败: ' + errMsg),
  });

  /**
   * 嵌入状态检查（已抽出到 useEmbeddingStatus hook）
   *
   * 当论文解析完成后，定期轮询检查其嵌入（向量化）状态。
   * pending 且从未 done 时清空状态。
   */
  const { embeddingStatus } = useEmbeddingStatus({
    paperId,
    parseStatus: paper?.parse_status,
  });

  // ========================================================================
  // ===== 智能滚动（已抽出到 useSmartScroll hook）=====
  // ========================================================================
  // 用户向上阅读时不打断；用户发新消息时重置跟随；流式更新时仅在底部附近跟随。
  const { isNearBottomRef, handleScroll } = useSmartScroll({
    messages,
    messageListRef,
    messagesEndRef,
  });

  /**
   * 保存统一模板数据到后端 settings
   *
   * 由模板管理弹窗（TemplateManagerModal）在用户点击"保存"时调用。
   * 完整流程：
   * 1. 将模板数据序列化为 JSON 字符串
   * 2. 通过 updateSettings API 持久化到后端 settings 表的 custom_prompt_templates 键
   * 3. 更新本地 templateData 状态（控制弹窗的数据源）
   * 4. 调用 registerTemplates 更新 ContextBridge 模块级变量
   *    （使 PdfViewer、ParagraphGuide 的模板查询立即生效，无需刷新页面）
   *
   * @param {object} data - 新的模板数据 { templates: [...] }
   */
  // handleSaveTemplates 已移至 useTemplateManager hook

  /**
   * 发送消息入口函数
   *
   * 作为 SlateInputWithSender 组件的发送回调，
   * 负责：追问线程路由、斜杠命令拦截、参数适配、调用 useChatSender 的 sendMessages
   *
   * 追问线程模式：
   * - 当 activeThread 不为 null 时，输入框处于追问线程模式
   * - 发送消息时路由到 handleThreadSendMessage 而非主对话的 sendMessages
   * - 追问不支持附件、网络搜索等高级功能，仅纯文本
   *
   * @param {string} text - 用户输入的文本内容
   * @param {Array} vibeCardRefs - VibeCard 引用列表（可选，暂未使用）
   * @param {Array} attachments - 附件列表（可选，图片/文件等）
   * @param {boolean} webSearchEnabled - 是否开启网络搜索（可选，默认 false）
   * @param {Array} fileRefs - 文件引用列表（可选，用于文件附件）
   */
  const handleSend = useCallback(async (text: string, vibeCardRefs: any[] = [], attachments: any[] = [], webSearchEnabled: boolean = false, fileRefs: any[] = [], contextSystemPrompt?: string) => {
    // 校验：流式回复中不允许重复发送（Phase 4: 从 store 读最新值，替代 streamingRef.current）
    if (getStreamingForPaper(paperId)) return;

    // ===== 追问线程模式路由 =====
    // 如果当前处于追问线程模式，将消息路由到线程发送函数
    if (activeThread) {
      const trimmed = text.trim();
      if (!trimmed) return; // 空消息不发送
      if (activeThread.subThreadId) {
        // 子线程（二级追问）：路由到子线程发送函数
        handleSubThreadSendMessage(activeThread.msgIdx, activeThread.threadId, activeThread.subThreadId, trimmed);
      } else {
        // 一级线程：路由到线程发送函数
        handleThreadSendMessage(activeThread.msgIdx, activeThread.threadId, trimmed);
      }
      return; // 线程发送完成后保持激活状态，方便用户继续追问
    }

    // ===== 斜杠命令拦截 =====
    // 在消息处理之前，检查是否为斜杠命令（/new、/resume 等）
    // 如果是斜杠命令，执行对应逻辑并提前返回（不作为普通消息发送）
    // 注意：斜杠命令检查必须在 streaming 检查之后，但在空消息检查之前

    const trimmed = text.trim(); // 用于斜杠命令匹配

    // 首先尝试使用 useSlashCommands hook 的 handleSlashCommand
    // 该 hook 处理 /new, /attachment, /model, /clear, /hide 命令
    if (handleSlashCommand(text)) return; // 命中斜杠命令，提前返回

    // /resume 命令：打开对话历史弹窗（由 ChatPanel 主组件处理）
    if (trimmed === '/resume') {
      void loadConversations().then((loadedConversations) => {
        NiceModal.show('conversation-history', {
          conversations: loadedConversations,
          onResume: handleResumeConversation,
          onDelete: handleDeleteConversation,
        });
      });
      return true;
    }

    // /prompt 命令：打开模板管理弹窗（由 ChatPanel 主组件处理）
    if (trimmed === '/prompt') {
      void NiceModal.show('template-manager', {
        templateData,
        onSave: handleSaveTemplates,
        agentType,
        customSystemPrompt,
        onCustomSystemPromptSave: handleCustomSystemPromptSave,
        contextPreviewData,
      });
      return true;
    }

    // 校验：文本和附件都为空时不发送
    const userMessage = text.trim(); // 获取输入文本并去除首尾空格
    if (!userMessage && (!attachments || attachments.length === 0)) return; // 文本和附件都为空时不发送

    // ===== 调用 useChatSender 的 sendMessages =====
    // 所有校验通过后，调用核心发送函数
    // contextSystemPrompt：来自 ContextBridge 模板的 system 段（如 PDF 选中翻译场景），
    // 透传给 useChatSender 在主对话路径 buildSystemPrompt 里作为单次 customSystemPrompt 覆盖默认 persona
    await sendMessages(
      text,                    // 用户输入的原始文本
      vibeCardRefs,            // VibeCard 引用列表
      attachments,             // 附件列表
      webSearchEnabled,        // 是否开启网络搜索
      quotedMessage,           // 引用的消息对象
      setQuotedMessage as any,        // 清除引用状态的函数（发送后清除引用）
      null,                    // 预留参数
      fileRefs,                // 文件引用列表
      contextSystemPrompt,     // 模板 system 段（可选，单次注入优先于全局 customSystemPrompt）
    );
  }, [
    paperId,                   // Phase 4: getStreamingForPaper(paperId) 用于防重复发送
    handleSlashCommand,        // 斜杠命令处理函数
    quotedMessage,             // 引用的消息
    sendMessages,              // 核心发送函数
    setQuotedMessage,          // 清除引用状态
    activeThread,              // 当前激活的追问线程
    handleThreadSendMessage,   // 线程发送函数
    handleSubThreadSendMessage, // 子线程发送函数
  ]);

  // ========================================================================
  // ===== 命令式 API：通过 useImperativeHandle 暴露给父组件 =====
  // ========================================================================

  /**
   * 暴露 sendMessage 和 injectContext 方法给父组件
   *
   * PaperReaderPage 通过 ref 调用这些方法，实现从外部触发消息发送。
   * 所有消息（PDF 右键、导览提问、手动输入）统一走 history，无差别处理。
   *
   * 依赖数组说明：
   * - collapsed: 面板折叠状态 prop，折叠时不允许发送消息
   * - streaming: 流式回复状态，AI 回复中时不允许重复发送
   * - handleSend: 实际执行消息发送的函数
   * 这些依赖变化时需要重新创建 ref 对象，确保总是使用最新的状态值
   */
  useImperativeHandle(ref, () => ({
    /**
     * 发送消息方法
     *
     * 由父组件调用，将文本直接作为用户消息发送到对话面板。
     * 在面板折叠、消息为空、或正在流式回复时不执行。
     *
     * @param {string} text - 要发送的消息文本
     */
    sendMessage: (text: string, opts?: { systemPrompt?: string }) => {
      if (collapsed || !text?.trim() || streaming) return; // 校验条件
      handleSend(text, [], [], false, [], opts?.systemPrompt); // 调用发送函数，透传可选 system 段
    },
    /**
     * 注入上下文 prompt 并发送到对话（两段式：system + user）
     *
     * 将模板生成的 { systemPrompt, userPrompt } 拆分发送：
     * - userPrompt 作为用户消息文本送入对话
     * - systemPrompt 作为单次 system 段透传到 useChatSender.buildSystemPrompt，
     *   在本次请求中覆盖默认 persona（优先级：单次 contextSystemPrompt > 全局 customSystemPrompt > 默认 persona）
     *
     * 通常由段落导览的"深入提问"按钮或 PDF 右键菜单调用。
     *
     * @param {{ prompt?: string; userPrompt?: string; systemPrompt?: string }} param - 两段 prompt（兼容旧调用方传 { prompt }）
     */
    injectContext: ({ prompt, userPrompt, systemPrompt }: { prompt?: string; userPrompt?: string; systemPrompt?: string }) => {
      const text = userPrompt ?? prompt; // 兼容：优先 userPrompt，回退到老字段 prompt
      if (collapsed || !text?.trim() || streaming) return; // 校验条件
      handleSend(text, [], [], false, [], systemPrompt); // 调用发送函数，透传可选 system 段
    },
  }), [collapsed, streaming, handleSend]);

  // ===== 消息级动作（已抽出到 useMessageActions hook）=====
  // delete / quote / thread activation / sub-thread activation / sub-thread collapse adapter
  const {
    handleDeleteMessage,
    handleQuoteMessage,
    handleActivateThreadForMessage,
    handleActivateSubThreadForMessage,
    handleSubThreadCollapseAdapter,
  } = useMessageActions({
    setMessages,
    persistMessages,
    setQuotedMessage,
    handleActivateThread,
    handleActivateSubThread,
    handleToggleSubThreadCollapse,
  });

  /**
   * 处理输入区域右键菜单
   *
   * 在输入区域右键时切换模型选择器的显示状态。
   * 默认显示输入框，右键后切换为模型选择下拉菜单。
   * 使用 useCallback 提取内联箭头函数以避免每次渲染时重新创建。
   *
   * @param {Event} e - 右键菜单事件
   */
  const handleInputContextMenu = useCallback((e: any) => {
    e.preventDefault(); // 阻止浏览器默认右键菜单
    setShowModelSelector((prev) => !prev); // 切换模型选择器显示状态
  }, []);

  // ========================================================================
  // ========== 对话历史弹窗数据加载 ==========
  // ========================================================================
  // 当弹窗打开时，自动从后端加载对话列表数据
  // useEffect 已在 useConversationManager hook 中处理，此处无需重复

  // ========================================================================
  // ===== 条件渲染：加载中状态 =====
  // ========================================================================
  if (loading) { // 如果正在加载历史消息
    return ( // 返回加载中 UI
      <div style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', height: '100%' }}> {/* 居中容器 */}
        <Spin /> {/* Ant Design 加载动画组件 */}
      </div>
    );
  }

  // ========================================================================
  // ===== 条件渲染：折叠状态 =====
  // ========================================================================
  // 折叠状态下不渲染任何内容（通过 Ctrl+右方向键切换）
  // 折叠时由父组件显示展开按钮
  if (collapsed) {
    return null;
  }

  // ===== 头部模型选择器相关计算 =====
  // 注意：displayModel、dropdownModelItems、selectedMenuKey、menuSelectedKeys
  // 已由 useModelConfig hook 提供，此处无需重复定义

  /**
   * 处理头部模型下拉菜单的选择（仅用于快速切换模型）
   *
   * 当用户在头部模型选择器中选择一个自定义配置时，
   * 将该配置转换为统一的模型切换参数格式，调用 handleModelChange。
   *
   * @param {{ key: string }} param - 下拉菜单选择事件，key 为配置 ID
   */
  const handleHeaderModelSelect = ({ key }: { key: string }) => {
    // 在自定义配置列表中查找选中的配置
    const config = customConfigs.find(c => c.id === key);
    if (config) {
      // 调用模型切换函数，传入标准化的参数格式
      handleModelChange({
        key: 'custom', // 标记为自定义模型
        label: formatCustomModelLabel(config.modelName || config.name), // 格式化显示标签
        configId: config.id, // 配置 ID
        config, // 完整配置对象
      });
    }
  };

  // ========================================================================
  // ===== 主 UI 渲染 =====
  // ========================================================================
  return ( // 返回主 UI
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}> {/* 纵向 flex 布局，全高 */}

      {/* ===== 隐藏的文件选择 input ===== */}
      {/* 仅供 /attachment 斜杠命令触发，用户输入 /attachment 后触发此 input 的 click 事件 */}
      {/* accept 属性列出了支持的文件类型：图片、PDF、Office 文档、代码文件、配置文件等 */}
      <input
        ref={attachmentInputRef} // ref 由 useSlashCommands hook 提供
        type="file"
        accept=".png,.jpg,.jpeg,.gif,.webp,.bmp,.svg,.pdf,.docx,.doc,.xlsx,.xls,.csv,.md,.txt,.py,.json,.js,.ts,.jsx,.tsx,.css,.scss,.html,.xml,.yaml,.yml,.toml,.sh,.bash,.zsh,.sql,.c,.cpp,.h,.java,.go,.rs,.rb,.r,.tex,.log,.env,.ini,.cfg,.conf,.vue,.svelte,.php,.swift,.kt,.graphql,.gql,.diff,.patch"
        onChange={handleAttachmentFileChange} // 文件选择回调
        style={{ display: 'none' }} // 隐藏 input 元素
      />

      {/* ================================================================== */}
      {/* ===== 消息列表区域（可滚动容器）===== */}
      {/* ================================================================== */}
      {/* 这是聊天的核心区域，包含所有消息、团队进度面板、搜索提示、嵌入状态提示等 */}
      <div ref={messageListRef} className="auto-scrollbar" style={{ flex: 1, overflow: 'auto', padding: 12, position: 'relative' }} onScroll={handleScroll} onClick={(e) => {
        // 点击消息列表非交互区域时取消追问线程激活状态并聚焦输入框
        // 跳过交互元素（按钮、链接等）和文本选区操作，避免干扰复制和按钮点击
        const interactiveSelector = 'a, button, input, textarea, select, [contenteditable="true"], [role="button"], [role="menuitem"]';
        if ((e.target as any).closest?.(interactiveSelector)) return; // 点击了交互元素，不处理
        const selection = window.getSelection(); // 获取当前文本选区
        if (selection && selection.toString().length > 0) return; // 用户正在选择文本，不处理
        handleDeactivateThread(); // 取消追问线程激活状态
        // 使用 setTimeout 延迟聚焦，避免在 click 事件同步处理期间被浏览器重置焦点
        setTimeout(() => { try { (slateInputRef.current as any)?.focus(); } catch {} }, 0);
      }}> {/* 消息列表区域，可滚动容器；onScroll 监听用户滚动行为，用于智能判断是否跟随 */}

        {/* ===== 搜索过滤提示横幅 ===== */}
        {/*
         * 当统一搜索栏的搜索目标为「聊天」且有搜索关键词时显示。
         * 告知用户当前处于搜索过滤模式，并显示匹配的消息数量。
         * 黄色警告色调与搜索高亮色系一致（rgba(250, 173, 20, ...)）
         */}
        {chatSearchKeyword.trim() && ( // 有搜索关键词时显示横幅
          <div style={{
            padding: '6px 12px', // 上下 6px，左右 12px 内边距
            background: 'var(--bg-warning-tint)', // 警告背景色
            borderBottom: '1px solid var(--border-warning)', // 底部警告边框
            fontSize: 12, // 小号字体
            color: 'var(--text-warning)', // 警告文字颜色
            marginBottom: 8, // 与消息列表的间距
            borderRadius: 4, // 轻微圆角
          }}>
            搜索 "{chatSearchKeyword.trim()}" — 找到 {filteredMessages.length} 条匹配消息 {/* 搜索关键词和匹配数量 */}
          </div>
        )}

        {/* ===== 嵌入状态提示横幅 ===== */}
        {/*
         * 当论文内容索引未完成时显示。
         * 告知用户当前论文的嵌入状态（pending/in_progress/done）。
         * 蓝色信息色调（使用全局次要背景色变量）
         */}
        {embeddingStatus && embeddingStatus.status !== 'done' && (
          <div style={{
            padding: '8px 12px',
            background: 'var(--bg-tertiary)', // 三级背景色，支持暗色主题
            borderRadius: '6px',
            margin: '4px 0',
            fontSize: '13px',
            color: 'var(--text-secondary)', // 次要文字颜色
            textAlign: 'center',
          }}>
            {/* 根据嵌入状态显示不同的提示文本 */}
            {embeddingStatus.status === 'pending'
              ? '论文内容等待索引...' // 等待状态
              : `论文内容索引中（${embeddingStatus.embedded}/${embeddingStatus.totalChunks}），检索功能暂不可用`} {/* 进行中状态，显示进度 */}
          </div>
        )}

        {/* ===== 消息列表条件渲染 ===== */}
        {/* 根据搜索状态和消息数量显示不同的内容 */}
        {filteredMessages.length === 0 && messages.length > 0 ? ( // 搜索过滤模式下无匹配结果（但原始消息不为空）
          // 搜索无结果时的空状态
          <Empty // Ant Design 空状态组件
            image={Empty.PRESENTED_IMAGE_SIMPLE} // 简化的空状态图片
            description={`未找到包含"${chatSearchKeyword.trim()}"的消息`} // 提示搜索无结果
            style={{ marginTop: 40 }} // 顶部间距
          />
        ) : filteredMessages.length === 0 && messages.length === 0 ? ( // 无消息（初始空状态）
          // 初始空状态：提示用户开始对话
          <div style={{
            position: 'absolute', // 绝对定位，撑满容器
            top: 0,
            left: 0,
            right: 0,
            bottom: 0,
            display: 'flex',
            alignItems: 'center', // 垂直居中
            justifyContent: 'center', // 水平居中
            pointerEvents: 'none', // 不拦截鼠标事件（让背景可以交互）
          }}>
            {/* 文字颜色：使用全局次要文字颜色变量 */}
            <span style={{ fontSize: 13, color: 'var(--text-secondary, #595959)', opacity: 0.6 }}>
              {/* 根据是否有选中的模型显示不同的占位文本 */}
              {displayModel ? `与 ${displayModel} 对话` : '与 AI 对话'}
            </span>
          </div>
        ) : ( // 有消息（可能是全部消息或搜索过滤后的消息）
          // 遍历过滤后的消息数组渲染每条消息
          filteredMessages.map((msg) => {
            // ===== 查找消息在原始 messages 数组中的索引 =====
            // 为什么需要原始索引？
            // - 删除消息的 handleDeleteMessage 函数使用原始 messages 数组的索引
            // - filteredMessages 是过滤后的子集，其索引与原始数组不一致
            // - 通过 Map 缓存查找（O(1)），确保操作正确的消息
            const originalIdx = messageIndexMap.get(msg) ?? -1; // 使用预构建的索引映射表查找（O(1)），找不到时返回 -1
            // 使用 msg.id 作为 React key（保证稳定性），回退到索引（向后兼容）
            const msgKey = msg.id ?? `idx-${originalIdx}`;
            return (
              // 渲染单条消息组件
              <ChatMessageItem
                key={msgKey} // React key：用于列表渲染的身份标识
                msg={msg} // 消息对象：包含 role、content、threads 等字段
                originalIdx={originalIdx} // 消息在原始数组中的索引
                streaming={streaming} // 是否正在流式回复中
                isLastMessage={originalIdx === messages.length - 1} // 是否是最后一条消息（用于流式输出效果）
                lastWebSearchSources={lastWebSearchSources} // 最近一次网络搜索的来源列表
                chatSearchKeyword={chatSearchKeyword} // 搜索关键词（用于高亮匹配文本）
                paperId={paperId} // 论文 ID
                onQuoteMessage={handleQuoteMessage} // 引用消息回调
                onDeleteMessage={handleDeleteMessage} // 删除消息回调
                onSearchResultClick={handleSearchResultClick} // 搜索结果点击回调
                // Phase 5: 线程相关 props
                threads={msg.threads || []} // 该消息关联的线程数组
                threadCount={msg.threads?.length || 0} // 线程数量（用于显示徽标）
                onCreateThread={handleCreateAndActivateThread} // 创建线程回调（自动激活）
                onDeleteThread={handleDeleteThread} // 删除线程回调
                onToggleThreadCollapse={handleToggleThreadCollapse} // 切换线程折叠回调
                onCollapseAll={handleCollapseAll} // 折叠所有线程回调
                onExpandAll={handleExpandAll} // 展开所有线程回调
                onThreadSendMessage={handleThreadSendMessage} // 线程内发送消息回调
                isThreadCollapsed={isThreadCollapsed} // 判断线程是否折叠的函数
                isMainStreaming={isMainStreaming} // 主对话是否正在流式
                isThreadStreaming={isThreadStreaming} // 是否有线程正在流式
                isThreadStreamingById={isThreadStreaming as any} // Phase 6: 判断指定线程是否在流式回复中 (threadId) => boolean
                onThreadMessageDelete={handleDeleteThreadMessage as any} // Phase 6: 线程内删除消息回调
                onThreadMessageEdit={handleEditThreadMessage as any} // Phase 6: 线程内编辑消息回调
                onActivateThread={handleActivateThreadForMessage(originalIdx) as any} // 追问线程激活回调（为每条消息创建独立的闭包）
                // 子线程相关 props
                onCreateAndActivateSubThread={handleCreateAndActivateSubThread} // 创建并激活子线程回调
                onActivateSubThread={handleActivateSubThreadForMessage(originalIdx) as any} // 激活子线程回调
                onSubThreadSendMessage={handleSubThreadSendMessage as any} // 子线程发送消息回调
                onDeleteSubThread={handleDeleteSubThread as any} // 删除子线程回调
                onToggleSubThreadCollapse={handleSubThreadCollapseAdapter as any} // 子线程折叠切换回调
                onDeleteSubThreadMessage={handleDeleteSubThreadMessage as any} // 子线程内删除消息回调
                onEditSubThreadMessage={handleEditSubThreadMessage as any} // 子线程内编辑消息回调
                onDeactivateThread={handleDeactivateThread} // 退出追问模式回调
              />
            );
          })
        )}
        {/* 滚动锚点：位于消息列表底部的空 div，用于 scrollIntoView 自动滚动 */}
        <div ref={messagesEndRef} />
      </div>

      {/* ================================================================== */}
      {/* ===== 输入区域 ===== */}
      {/* ================================================================== */}
      {/* 包含引用预览条、追问指示条、模型选择器/输入框 */}
      {/* 右键可在聊天输入框和模型选择器之间切换 */}
      <div // 输入区域容器
        className={streaming ? 'ai-chat-input-streaming' : undefined} // 流式回复时添加特殊样式类
        style={{
          padding: 12, // 内边距
          borderTop: streaming
            ? '2px solid var(--color-primary)' // 流式回复时：2px 主题色分隔线（视觉反馈）
            : '1px solid var(--border-color)', // 非流式时：1px 普通分隔线
          flexShrink: 0, // 防止输入区域被压缩（保持固定高度）
          transition: 'border-color 0.3s, border-width 0.15s', // 分隔线颜色和宽度过渡动画
        }}
        onContextMenu={handleInputContextMenu} // 右键切换模型选择器
        onClick={(e) => {
          // 点击输入区域的非交互区域时聚焦输入框
          const interactiveSelector = 'a, button, input, textarea, select, [contenteditable="true"], [role="button"], [role="menuitem"]';
          if ((e.target as any).closest?.(interactiveSelector)) return; // 点击了交互元素，不处理
          setTimeout(() => { try { (slateInputRef.current as any)?.focus(); } catch {} }, 0); // 延迟聚焦输入框
        }}
      >

        {/* ===== 引用消息预览条 ===== */}
        {/* 当用户右键某条消息并选择"引用消息"后，此预览条会显示在输入框上方 */}
        {/* 预览条包含：被引用消息的角色标签 + 内容摘要（截断显示） + 操作按钮 + 取消按钮 */}
        {/* 发送消息后或点击取消按钮时，引用状态会被清除，预览条消失 */}
        {quotedMessage && ( // 仅在存在引用消息时渲染预览条
          <div // 引用预览条容器
            style={{
              marginBottom: 8, // 与下方输入框的间距
              padding: '6px 10px', // 内边距：上下 6px，左右 10px
              background: 'var(--bg-secondary, #f5f5f5)', // 背景色：使用 CSS 变量（支持暗色主题），fallback 为浅灰色
              borderRadius: 6, // 圆角
              borderLeft: '3px solid var(--color-primary)', // 左侧强调色竖线，视觉上标识这是一条引用
              display: 'flex', // flex 布局，使内容与关闭按钮水平排列
              alignItems: 'center', // 垂直居中对齐
              gap: 8, // 内容与关闭按钮之间的间距
            }}
          >
            {/* 引用内容区域：角色标签 + 消息摘要 */}
            <div style={{
              flex: 1, // 占据剩余空间，将关闭按钮推到右侧
              minWidth: 0, // 允许内容收缩，使 textOverflow 生效
              fontSize: 12, // 字号 12px，比正文略小，表明这是引用而非正文
              color: 'var(--text-secondary, #595959)', // 文字颜色：使用全局次要文字颜色变量
              lineHeight: 1.4, // 行高
            }}>
              {/* 角色标签：显示被引用消息是来自"AI"还是"用户" */}
              <span className="accent-title-text">
                引用了{quotedMessage.role === 'assistant' ? 'AI' : '用户'}的消息
              </span>
              {/* 消息摘要：截取前 80 个字符，超出部分用省略号表示 */}
              <div style={{
                marginTop: 2,
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap', // 不换行，超出部分显示省略号
              }}>
                {quotedMessage.content.slice(0, 80)} {/* 截取前 80 个字符 */}
                {quotedMessage.content.length > 80 ? '...' : ''} {/* 超出部分显示省略号 */}
              </div>
            </div>

            {/* Phase 6: "转为追问"按钮 */}
            {/* 仅在引用 AI 消息时显示，因为追问只针对 AI 回答 */}
            {/* 点击后将引用消息转为追问线程的锚点，输入框切换到追问模式 */}
            {quotedMessage.role === 'assistant' && quotedMessage.originalIdx !== undefined && (
              <div
                style={{
                  display: 'inline-flex',
                  alignItems: 'center',
                  gap: 4, // 图标和文字间距
                  padding: '2px 8px',
                  fontSize: 11, // 小号字体
                  color: 'var(--color-primary)', // 主题色文字
                  background: 'var(--hover-overlay)', // 悬浮遮罩背景
                  borderRadius: 4, // 圆角
                  cursor: 'pointer', // 鼠标指针变手型
                  transition: 'all 0.2s', // 过渡动画
                  flexShrink: 0, // 防止被压缩
                }}
                onMouseEnter={(e) => {
                  // 鼠标悬浮时加深背景
                  e.currentTarget.style.background = 'var(--active-overlay)';
                }}
                onMouseLeave={(e) => {
                  // 鼠标离开时恢复背景
                  e.currentTarget.style.background = 'var(--hover-overlay)';
                }}
                onClick={() => {
                  // 处理"转为追问"操作
                  if (quotedMessage.role !== 'assistant') return; // 安全检查：只处理 AI 消息
                  const msgIdx = quotedMessage.originalIdx; // 获取消息索引
                  if (msgIdx === undefined || msgIdx < 0) return; // 索引校验

                  // 获取被引用的消息对象
                  const targetMsg = messages[msgIdx];
                  if (!targetMsg) return; // 消息不存在时返回

                  // 生成消息内容的简单 hash 作为 fingerprint
                  // 使用内容的前 50 个字符作为简化指纹，用于唯一标识线程
                  const contentFingerprint = `msg-${msgIdx}-${targetMsg.content.slice(0, 50)}`;

                  // 调用创建线程回调，传递指纹和锚定文本
                  handleCreateThread(msgIdx, {
                    fingerprint: contentFingerprint, // 线程唯一标识
                    anchorText: quotedMessage.content.slice(0, 100), // 锚定文本（截取前 100 字符）
                  });

                  // 清除引用状态，预览条消失
                  setQuotedMessage(null);
                }}
              >
                <MessageOutlined style={{ fontSize: 10 }} /> {/* 消息图标 */}
                <span>转为追问</span> {/* 按钮文字 */}
              </div>
            )}

            {/* 取消引用按钮：点击后清除 quotedMessage 状态，预览条消失 */}
            <CloseOutlined // 使用 antd 的关闭图标
              style={{
                fontSize: 12, // 图标大小 12px
                color: 'var(--text-secondary, #595959)', // 图标颜色：使用全局次要文字颜色变量
                cursor: 'pointer', // 鼠标指针变为手型，提示可点击
                flexShrink: 0, // 防止图标被压缩
              }}
              onClick={() => setQuotedMessage(null)} // 点击时清除引用状态
            />
          </div>
        )}

        {/* ===== 追问线程上下文指示条 ===== */}
        {/* 当用户点击某个追问线程后，此指示条显示在输入框上方 */}
        {/* 提示用户当前输入框处于追问模式，显示锚定文本和消息计数 */}
        {/* 一级线程用蓝色竖线标识，二级子线程用绿色竖线标识 */}
        {activeThreadInfo && (
          <div style={{
            marginBottom: 8, // 与下方输入框的间距
            padding: '6px 10px', // 内边距
            background: 'var(--bg-secondary, #f5f5f5)', // 背景色
            borderRadius: 6, // 圆角
            borderLeft: activeThreadInfo.isSubThread
              ? '3px solid var(--status-success)' // 子线程：绿色竖线
              : '3px solid var(--color-primary)', // 一级线程：强调色竖线
            display: 'flex', // flex 布局
            alignItems: 'center', // 垂直居中
            gap: 8, // 元素间距
          }}>
            {/* 线程图标：颜色与左侧竖线一致 */}
            <MessageOutlined style={{
              fontSize: 12,
              color: activeThreadInfo.isSubThread
                ? 'var(--status-success)' // 子线程：绿色
                : 'var(--color-primary)', // 一级线程：强调色
              flexShrink: 0 // 防止被压缩
            }} />
            {/* 线程信息：类型标签 + 锚定文本 + 消息计数 */}
            <div style={{
              flex: 1, // 占据剩余空间
              minWidth: 0, // 允许收缩
              fontSize: 12,
              color: 'var(--text-secondary, #595959)', // 次要文字颜色
              lineHeight: 1.4,
              overflow: 'hidden',
              textOverflow: 'ellipsis', // 超出部分显示省略号
              whiteSpace: 'nowrap', // 不换行
            }}>
              {/* 线程类型标签和锚定文本 */}
              {activeThreadInfo.isSubThread ? '二级追问' : '追问'} — 「{(activeThreadInfo as any).anchorText || '选定文本'}」
              {/* 消息计数：当前消息数 / 最大消息数 */}
              {activeThreadInfo.messageCount > 0 && (
                <span style={{ marginLeft: 6, fontSize: 10, color: 'var(--text-tertiary, #8c8c8c)' }}>
                  {activeThreadInfo.messageCount} / {activeThreadInfo.isSubThread ? MAX_SUBTHREAD_MESSAGES : MAX_THREAD_MESSAGES}
                </span>
              )}
            </div>
            {/* 关闭按钮：退出追问模式 */}
            <CloseOutlined
              style={{
                fontSize: 12,
                color: 'var(--text-secondary, #595959)',
                cursor: 'pointer', // 鼠标指针变手型
                flexShrink: 0,
              }}
              onClick={handleDeactivateThread} // 点击退出追问模式
            />
          </div>
        )}

        {/* ===== 模型选择器 / 输入框切换区域 ===== */}
        {/* 始终挂载两个组件，通过 CSS display 切换可见性，避免 SlateInputWithSender 因卸载而丢失输入状态 */}

        {/* 模型选择器：右键输入区域时显示，替换输入框 */}
        {showModelSelector && (
          <Dropdown
            placement="topLeft" // 向上弹出
            getPopupContainer={(triggerNode: any) =>
              (triggerNode?.ownerDocument || document).body // 弹出菜单挂载到 body，避免被容器裁剪
            }
            menu={{
              items: dropdownModelItems, // 模型菜单项（由 useModelConfig 生成）
              onClick: handleHeaderModelSelect, // 选择模型后的回调
              className: 'model-dropdown-menu', // 自定义样式类
              selectedKeys: menuSelectedKeys, // 当前选中的菜单项（高亮显示）
            }}
            trigger={['click']} // 点击触发
          >
            <Button
              size="small"
              type="text" // 无边框文本按钮
              style={{
                padding: '0 4px',
                display: 'inline-flex',
                alignItems: 'center',
                minWidth: 0,
                maxWidth: '100%', // 限制最大宽度
              }}
              title={zoteroL10n('vibe-ai-chat-current-model-title', { model: displayModel || 'AI' })} // 鼠标悬浮提示
            >
              <span style={{
                fontWeight: 500,
                fontSize: 14,
                overflow: 'hidden',
                textOverflow: 'ellipsis', // 超出显示省略号
                whiteSpace: 'nowrap', // 不换行
                minWidth: 0,
              }}>
                {displayModel || 'AI'} {/* 显示当前模型名称 */}
              </span>
            </Button>
          </Dropdown>
        )}

        {/* 输入框区域：默认显示，右键时隐藏 */}
        <div style={showModelSelector ? { display: 'none' } : undefined}>
          <SlateInputWithSender
            ref={slateInputRef} // ref 用于调用 openConfigModal 等命令式方法
            onSubmit={handleSend as any} // 消息提交回调
            loading={
              // 加载状态：追问模式下使用线程的流式状态，普通模式使用主对话的流式状态
              activeThread
                ? (activeThreadInfo ? isThreadStreaming(activeThread.threadId) || activeThreadInfo.isAtLimit : false)
                : streaming
            }
            placeholder={
              // 占位文本：追问模式下显示追问提示，普通模式显示发送提示
              activeThread
                ? (activeThreadInfo?.isAtLimit ? '已达到追问消息上限' : '继续追问... (Enter 发送)')
                : 'Enter 发送, Shift+Enter 换行'
            }
            onVibeCardInsert={(insertFn: any) => { // 注册 VibeCard 插入方法
              // 将插入函数保存到 ref 上，供 injectContext 使用
              if (slateInputRef.current) {
                slateInputRef.current.insertVibeCard = insertFn;
              }
            }}
            onModelChange={handleModelChange as any} // 模型切换回调
            currentModel={currentModel as any} // 当前选中的模型信息
            visionCapable={!activeThread} // 追问模式下禁用多模态（追问面板未实现附件，文本模型的错误由后端 SSE 推回）
            customConfigs={customConfigs} // 自定义模型配置列表
            onSaveConfigs={handleSaveConfigs as any} // 保存配置回调
            selectedConfigId={selectedConfigId} // 当前选中的配置 ID
            webSearchEnabled={activeThread ? false : webSearchEnabled} // 追问模式下禁用网络搜索
            onWebSearchToggle={handleWebSearchToggle} // Web 搜索开关切换回调
            threadContextInjection={threadContextInjection} // 线程上下文注入开关状态
            onThreadContextInjectionToggle={handleThreadContextInjectionToggle} // 线程注入切换回调
            slashCommands={slashCommandList} // 斜杠命令列表（用于输入框的自动补全提示展示）
          />
        </div>
      </div>

      {/* ================================================================== */}
      {/* ===== 弹窗组件区域 ===== */}
      {/* 模板管理、对话历史、图片灯箱等弹窗已迁移到 NiceModal 命令式调用 ===== */}
      {/* ================================================================== */}

      {/* ===== 工具审批弹窗（通过 NiceModal 命令式调用） ===== */}

      {/* ===== 委派审批弹窗（通过 NiceModal 命令式调用） ===== */}
    </div>
  );
}); // forwardRef 结束

// 默认导出 ChatPanel 组件，供父组件通过 import ChatPanel from '...' 引入
export default ChatPanel;
