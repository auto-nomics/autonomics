/**
 * SSE Event Types for Autonomics Agent Chat
 *
 * Autonomics Agent 聊天的 SSE 事件类型定义
 *
 * 标准化的 SSE 事件协议，用于 Agent 流式响应。
 * 包含：
 * - Start/Content/ToolCall/ToolResult/Error/Done/Ping 事件
 * - 类型安全的事件负载定义
 * - 心跳机制
 *
 * @module types/sse
 */

// ============================================================================
// 单 Agent SSE 事件类型
// ============================================================================

/**
 * SSE 事件类型枚举
 */
export enum SseEventType {
  /** 会话开始 */
  START = 'start',
  /** 文本内容增量 */
  CONTENT = 'content',
  /** 工具调用已发起 */
  TOOL_CALL = 'tool_call',
  /** 工具调用结果 */
  TOOL_RESULT = 'tool_result',
  /** 发生错误 */
  ERROR = 'error',
  /** 流式传输完成 */
  DONE = 'done',
  /** 心跳/保活 ping */
  PING = 'ping',
  /** 新对话已建(携带 conversation_id,不应被误判为 DONE) */
  CONVERSATION_CREATED = 'conversation_created',
}

/**
 * 基础 SSE 事件接口
 */
export interface SseEvent<T = unknown> {
  /** 事件类型 */
  type: SseEventType;
  /** 事件时间戳（ISO 8601 格式） */
  timestamp?: string;
  /** 事件特定负载 */
  data: T;
}

/**
 * Start 事件 - Agent 会话开始时发送
 */
export interface SseEventStart {
  type: typeof SseEventType.START;
  data: {
    /** 会话/对话 ID */
    session_id?: string;
    /** 使用的模型 */
    model?: string;
    /** 会话开始时间戳 */
    started_at?: string;
  };
}

/**
 * Content 事件 - 流式响应的文本增量
 */
export interface SseEventContent {
  type: typeof SseEventType.CONTENT;
  data: {
    /** 文本增量内容 */
    text: string;
    /** 是否为最后一个分块 */
    done?: boolean;
  };
}

/**
 * Tool Call 事件 - Agent 发起工具调用时
 */
export interface SseEventToolCall {
  type: typeof SseEventType.TOOL_CALL;
  data: {
    /** 工具使用 ID */
    tool_use_id: string;
    /** 工具名称 */
    name: string;
    /** 工具输入参数 */
    input: Record<string, unknown>;
    /** 此工具调用在序列中的索引 */
    index?: number;
  };
}

/**
 * Tool Result 事件 - 工具调用的结果
 */
export interface SseEventToolResult {
  type: typeof SseEventType.TOOL_RESULT;
  data: {
    /** 工具使用 ID（与对应的 tool_call 匹配） */
    tool_use_id: string;
    /** 工具执行是否有错误 */
    is_error?: boolean;
    /** 结果内容预览 */
    preview?: string;
    /** 完整结果内容（可能被截断） */
    content?: string;
  };
}

/**
 * Error 事件 - 流式传输过程中发生错误
 */
export interface SseEventError {
  type: typeof SseEventType.ERROR;
  data: {
    /** 错误消息 */
    message: string;
    /** 错误代码（可选） */
    code?: string;
    /** 错误详情 */
    details?: unknown;
  };
}

/**
 * Done 事件 - 流式传输成功完成
 */
export interface SseEventDone {
  type: typeof SseEventType.DONE;
  data: {
    /** 最终消息 ID */
    message_id?: string;
    /** 对话/会话 ID */
    conversation_id?: string;
    /** Token 使用统计 */
    usage?: {
      /** 输入 token 数(总 prompt tokens,含 cache 命中部分) */
      input_tokens?: number;
      /** 输出 token 数 */
      output_tokens?: number;
      /** Prompt cache 命中的 token 数(input_tokens 的子集) */
      cache_read_input_tokens?: number;
      /** 总 token 数 */
      total_tokens?: number;
    };
  };
}

/**
 * Ping 事件 - 心跳，保持连接活跃
 */
export interface SseEventPing {
  type: typeof SseEventType.PING;
  data: {
    /** 服务器时间戳 */
    server_time: string;
    /** 序列号（可选） */
    seq?: number;
  };
}

/**
 * ConversationCreated 事件 - 新对话已建,后端创建 conversation 后立即推送给前端
 *
 * 与 Done 严格区分:本事件**不**结束流,只是把新对话的 id 通知前端。
 * 早期版本曾把它误映射成 SseEventType.DONE,导致新对话流被提前关闭。
 */
export interface SseEventConversationCreated {
  type: typeof SseEventType.CONVERSATION_CREATED;
  data: {
    /** 新建的对话 ID */
    conversation_id: string;
  };
}

/**
 * 所有可能的 SSE 事件的联合类型
 */
export type AgentSseEvent =
  | SseEventStart
  | SseEventContent
  | SseEventToolCall
  | SseEventToolResult
  | SseEventError
  | SseEventDone
  | SseEventPing
  | SseEventConversationCreated;

/**
 * 向后兼容的旧事件名映射
 * 将旧事件名映射到新的标准化名称
 */
export const LEGACY_EVENT_MAP: Record<string, SseEventType> = {
  // 旧 agent 事件
  text_delta: SseEventType.CONTENT,
  tool_call_start: SseEventType.TOOL_CALL,
  tool_call_result: SseEventType.TOOL_RESULT,
  conversation_created: SseEventType.CONVERSATION_CREATED,
  // 标准事件
  start: SseEventType.START,
  content: SseEventType.CONTENT,
  tool_call: SseEventType.TOOL_CALL,
  tool_result: SseEventType.TOOL_RESULT,
  error: SseEventType.ERROR,
  done: SseEventType.DONE,
  ping: SseEventType.PING,
};

/**
 * SSE 事件类型守卫函数
 */
export function isSseEventContent(event: AgentSseEvent): event is SseEventContent {
  return event.type === SseEventType.CONTENT;
}

export function isSseEventToolCall(event: AgentSseEvent): event is SseEventToolCall {
  return event.type === SseEventType.TOOL_CALL;
}

export function isSseEventToolResult(event: AgentSseEvent): event is SseEventToolResult {
  return event.type === SseEventType.TOOL_RESULT;
}

export function isSseEventError(event: AgentSseEvent): event is SseEventError {
  return event.type === SseEventType.ERROR;
}

export function isSseEventDone(event: AgentSseEvent): event is SseEventDone {
  return event.type === SseEventType.DONE;
}

export function isSseEventPing(event: AgentSseEvent): event is SseEventPing {
  return event.type === SseEventType.PING;
}

export function isSseEventConversationCreated(
  event: AgentSseEvent,
): event is SseEventConversationCreated {
  return event.type === SseEventType.CONVERSATION_CREATED;
}

// ============================================================================
// (removed) Multi-Agent Team SSE 事件类型 — 已随多 Agent team 模式移除
// ============================================================================

/**
 * 将原始 SSE 事件数据解析为类型化事件对象
 *
 * 未识别的事件名(既不在 LEGACY_EVENT_MAP,也不在 SseEventType 中)返回 null,
 * 调用方应跳过——而不是把 eventName 强制转成 SseEventType,那样会让任何
 * 后端新增的事件被静默吞下或误判(历史 bug:`conversation_created` 曾被
 * 错误映射为 DONE 导致新对话流提前关闭)。
 */
export function parseSseEvent(eventName: string, rawData: unknown): AgentSseEvent | null {
  const eventType = LEGACY_EVENT_MAP[eventName];
  if (eventType === undefined) {
    console.warn(`[SSE] Unknown event type: ${eventName}`);
    return null;
  }

  switch (eventType) {
    case SseEventType.START:
      return { type: SseEventType.START, data: (rawData || {}) as SseEventStart['data'] };
    case SseEventType.CONTENT:
      return { type: SseEventType.CONTENT, data: (rawData || {}) as SseEventContent['data'] };
    case SseEventType.TOOL_CALL:
      return { type: SseEventType.TOOL_CALL, data: (rawData || {}) as SseEventToolCall['data'] };
    case SseEventType.TOOL_RESULT:
      return { type: SseEventType.TOOL_RESULT, data: (rawData || {}) as SseEventToolResult['data'] };
    case SseEventType.ERROR:
      return { type: SseEventType.ERROR, data: (rawData || {}) as SseEventError['data'] };
    case SseEventType.DONE:
      return { type: SseEventType.DONE, data: (rawData || {}) as SseEventDone['data'] };
    case SseEventType.PING:
      return { type: SseEventType.PING, data: (rawData || {}) as SseEventPing['data'] };
    case SseEventType.CONVERSATION_CREATED:
      return {
        type: SseEventType.CONVERSATION_CREATED,
        data: (rawData || {}) as SseEventConversationCreated['data'],
      };
    default:
      console.warn(`[SSE] Unhandled SseEventType: ${eventType}`);
      return null;
  }
}
