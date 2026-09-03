/**
 * TransactionNotifier 类型定义
 *
 * 定义事务感知通知系统的所有类型约束。
 */

// ========== 通知类型与事件 ==========

/** 可通知的对象类型，按域模型划分 */
export type NotifierType =
  | 'library'
  | 'folder'
  | 'paper'
  | 'annotation'
  | 'tag'
  | 'conversation';

/** 对象上发生的事件类型 */
export type NotifierEvent =
  | 'add'
  | 'modify'
  | 'remove'
  | 'move'
  | 'delete'
  | 'trash';

/** 所有合法类型（运行时校验用） */
export const VALID_TYPES: readonly NotifierType[] = [
  'library',
  'folder',
  'paper',
  'annotation',
  'tag',
  'conversation',
];

/** 所有合法事件（运行时校验用） */
export const VALID_EVENTS: readonly NotifierEvent[] = [
  'add',
  'modify',
  'remove',
  'move',
  'delete',
  'trash',
];

// ========== 队列结构 ==========

/** 队列中单个 (type, event) 的条目 */
export interface QueueEntry {
  ids: string[];
  data: Record<string, unknown>;
}

/**
 * 事件队列：type → event → QueueEntry
 *
 * 结构: { paper: { add: { ids: [...], data: {} }, modify: { ... } } }
 */
export type EventQueue = Partial<
  Record<NotifierType, Partial<Record<NotifierEvent, QueueEntry>>>
>;

// ========== 观察者 ==========

/** 通知观察者接口 */
export interface NotifierObserver {
  notify(
    event: NotifierEvent,
    type: NotifierType,
    ids: string[],
    extraData: Record<string, unknown>,
  ): void | Promise<void>;
}

/** 已注册观察者的内部记录 */
export interface ObserverRecord {
  ref: NotifierObserver;
  types: NotifierType[] | null; // null = 监听所有类型
  priority: number;
}

/**
 * 存在性检查函数 —— 用于过滤已删除对象的 modify 事件
 * 返回 true 表示对象仍然存在
 */
export type ExistenceChecker = (
  type: NotifierType,
  id: string,
) => boolean | Promise<boolean>;
