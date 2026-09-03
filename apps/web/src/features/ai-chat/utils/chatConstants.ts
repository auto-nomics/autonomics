/**
 * AI 聊天模块常量配置
 *
 * 本模块集中管理聊天功能相关的常量配置。
 * 包含 token 预算、默认配置等静态值。
 *
 * @module ai-chat/utils/chatConstants
 */

/**
 * Token 预算默认配置常量
 *
 * 定义各 AI 模型的最大可用 token 预算（用于发送给 API 的消息）。
 * 这个预算需要预留给响应，所以不是模型完整的 context window 大小。
 * 这是前端默认值，实际使用时会从后端 API 获取最新配置。
 *
 * 为什么需要这个配置？
 * - 不同模型的 context window 差异巨大（4K - 200K+）
 * - 需要预留空间给 AI 的回复，避免超出限制
 * - 前端需要根据预算决定注入多少上下文
 * - 提供 fallback 值，当 API 获取失败时使用
 *
 * 预留策略：
 * - 小模型（如 GPT-3.5）：预算 3000，预留 1000 给响应
 * - 中模型（如 GPT-4）：预算 6000，预留 2000 给响应
 * - 大模型（如 GPT-4o/Claude）：预算 100K-180K，预留足够空间
 */
export const DEFAULT_TOKEN_BUDGETS = {
  // GPT 系列模型
  'gpt-3.5-turbo': 3000,    // GPT-3.5 Turbo：4K context，预算 3000（预留 1000 给响应）
  'gpt-4': 6000,            // GPT-4：8K context，预算 6000（预留 2000 给响应）
  'gpt-4o': 100000,         // GPT-4o：128K context，预算 100K（预留 28K 给响应）
  'gpt-4o-mini': 100000,    // GPT-4o Mini：128K context，预算 100K

  // Claude 3 系列模型
  'claude-3-opus': 180000,  // Claude 3 Opus：200K context，预算 180K
  'claude-3-sonnet': 180000, // Claude 3 Sonnet：200K context，预算 180K
  'claude-3-haiku': 180000,  // Claude 3 Haiku：200K context，预算 180K

  // Claude 3.5 系列模型
  'claude-3.5-sonnet': 180000, // Claude 3.5 Sonnet：200K context，预算 180K
  'claude-3.5-haiku': 180000,  // Claude 3.5 Haiku：200K context，预算 180K

  // GPT-4.1 系列: 1M context window，预算 50%（大上下文模型刻意保守，避免"lost in the middle"效应和过高成本）
  'gpt-4.1': 500000,         // GPT-4.1：1M context，预算 500K
  'gpt-4.1-mini': 500000,    // GPT-4.1 Mini：1M context，预算 500K
  'gpt-4.1-nano': 500000,    // GPT-4.1 Nano：1M context，预算 500K

  // o3/o4 推理系列: 200K context window，预算 70%
  'o3-mini': 140000,         // o3 Mini：200K context，预算 140K
  'o4-mini': 140000,         // o4 Mini：200K context，预算 140K

  // DeepSeek 系列: 128K context window，预算 78%
  'deepseek-chat': 100000,   // DeepSeek Chat：128K context，预算 100K
  'deepseek-reasoner': 100000, // DeepSeek Reasoner：128K context，预算 100K

  // Gemini 系列: 1M context window，预算 50%（大上下文模型刻意保守）
  'gemini-2.0-flash': 500000,  // Gemini 2.0 Flash：1M context，预算 500K
  'gemini-2.5-pro': 500000,    // Gemini 2.5 Pro：1M context，预算 500K
  'gemini-2.5-flash': 500000,  // Gemini 2.5 Flash：1M context，预算 500K
};

/**
 * 消息角色常量
 *
 * 定义聊天消息的角色类型，用于标识消息的发送者。
 */
export const MESSAGE_ROLES = {
  USER: 'user',       // 用户消息
  ASSISTANT: 'assistant', // AI 助手消息
  SYSTEM: 'system',   // 系统提示消息（不直接显示给用户）
};

/**
 * 消息状态常量
 *
 * 定义消息的处理状态，用于 UI 显示和状态管理。
 */
export const MESSAGE_STATUS = {
  PENDING: 'pending',     // 待发送
  SENDING: 'sending',     // 发送中
  STREAMING: 'streaming', // 流式接收中
  COMPLETED: 'completed', // 完成
  FAILED: 'failed',       // 失败
};

/**
 * 附件分类常量
 *
 * 定义附件的分类类型，用于文件上传和消息构建。
 */
export const ATTACHMENT_CATEGORIES = {
  IMAGE: 'image',   // 图片附件（base64 编码）
  TEXT: 'text',     // 文本文件附件
  BINARY: 'binary', // 二进制文件附件（需后端转换）
};

/**
 * 上下文层级常量
 *
 * 定义分层上下文注入的层级，用于构建系统提示词。
 */
export const CONTEXT_LAYERS = {
  LAYER_1_PAPER_INFO: 'layer1',     // Layer 1：论文基础信息（标题、作者、摘要）
  LAYER_2_INJECTED: 'layer2',       // Layer 2：用户注入的上下文（选中文本、段落导览）
  LAYER_3_PARAGRAPH_GUIDE: 'layer3', // Layer 3：段落导览（按 section 分块）
  LAYER_4_RELEVANCE: 'layer4',      // Layer 4：相关性增强（章节语义）
};

/**
 * API 提供商常量
 *
 * 定义支持的 AI API 提供商。
 */
export const API_PROVIDERS = {
  OPENAI: 'openai',     // OpenAI API
  ANTHROPIC: 'anthropic', // Anthropic (Claude) API
};

/**
 * 流式响应事件类型常量
 *
 * 定义 SSE (Server-Sent Events) 流式响应的事件类型。
 */
export const SSE_EVENT_TYPES = {
  DATA: 'data',       // 数据块
  ERROR: 'error',     // 错误
  DONE: 'done',       // 完成
};

/**
 * 独立聊天 ID 哨兵值
 *
 * 用于 AI Chat 首页（非论文关联）的对话持久化。
 * 后端 Conversation 模型的 itemId 是普通 String，无 FK 约束，
 * 因此可以安全使用此哨兵值作为 itemId 来存储独立对话。
 */
export const STANDALONE_CHAT_ID = '__standalone__';
