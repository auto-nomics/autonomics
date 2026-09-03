/**
 * 聊天消息的共享 base interface。
 *
 * 5 个 hook（useChatStore / useActiveThread / useThreadManager /
 * useChatSender / threadUtils.ChatMessageLike）各自有 ChatMessage 接口，
 * 此前是分散定义。本文件抽出公共字段（id / role / content / threads）作为
 * 单一来源，各 hook 通过 `extends ChatMessageBase` + 字段 narrow 表达
 * 自己的约束（如 id 必填、content: string、threads: 本地 ThreadData[]）。
 *
 * ChatMessageLike（threadUtils.ts）对 role/content 做了 widen，无法用
 * extends 表达，独立保留但注释引用本 base。
 *
 * 设计参考：原 useChatStore.ts 注释
 * "feature 内部仍用各自的 ChatMessage 接口，但都 structurally compatible"
 */

/** 共享 base：所有 ChatMessage 变体的字段集合（最宽松形态） */
export interface ChatMessageBase {
  id?: string;
  role: 'user' | 'assistant' | 'system';
  content: string | unknown[];
  threads?: unknown[];
}

/** Store 入口的"通过类型"，含索引签名以承载业务字段（paperId / agentType / blocks 等） */
export interface ChatMessage extends ChatMessageBase {
  [key: string]: unknown;
}
