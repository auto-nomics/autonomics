/**
 * Autonomics Event Bus - 类型安全的事件总线系统
 *
 * 提供发布/订阅机制，用于组件间的松耦合通信。
 * 使用 TypeScript 类型系统确保事件载荷的类型安全。
 *
 * 设计原则：
 * 1. 事件定义即类型约束 - 每个事件都有明确的载荷类型
 * 2. 单一事件总线 - 全局单例，便于追踪和调试
 * 3. 自动内存管理 - 订阅者退订时自动清理
 *
 * 与 useBusStore 的区别：
 * - bus/index.ts: 核心事件总线，可在任何 TypeScript 代码中使用（不依赖 React）
 * - useBusStore: React 集成层，提供 hooks 和 Zustand 状态管理，仅在 React 组件中使用
 *
 * 使用场景：
 * - 在 React 组件中: 使用 useBusStore 中的 useBusSubscription hook
 * - 在非 React 代码中: 直接使用 JayreadBus.emit/on/off
 *
 * @example 在 React 组件中使用
 * import { useBusSubscription } from '@/stores/useBusStore';
 * useBusSubscription('paper.opened', (payload) => console.log(payload));
 *
 * @example 在非 React 代码中使用
 * import { JayreadBus } from '@/bus';
 * JayreadBus.on('paper.opened', (payload) => console.log(payload));
 */

import { create } from 'zustand';

// ========== 类型定义 ==========

/**
 * 事件载荷接口（用于定义具体事件）
 */
export interface EventPayload {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  [key: string]: any;
}

/**
 * 事件定义
 */
export interface EventDefinition<P extends EventPayload = EventPayload> {
  name: string;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  schema?: any; // 保留用于 zod 集成
}

/**
 * 事件监听器类型
 */
export type EventListener<P extends EventPayload = EventPayload> = (payload: P) => void;

/**
 * 退订函数类型
 */
export type Unsubscribe = () => void;

/**
 * 历史点（用于 Undo/Redo）
 */
interface HistoryPoint<T extends EventPayload = EventPayload> {
  timestamp: number;
  payload: T;
}

// ========== 事件定义 ==========

/**
 * 论文相关事件
 */
export const PaperEvents = {
  Opened: { name: 'paper.opened', schema: null as { paperID: string } | null },
  Closed: { name: 'paper.closed', schema: null as { paperID: string } | null },
} as const;

/**
 * 高亮标注相关事件
 */
export const AnnotationEvents = {
  Created: { name: 'annotation.created', schema: null as { paperID: string; highlight: HighlightPayload } | null },
  Updated: { name: 'annotation.updated', schema: null as { paperID: string; highlight: HighlightPayload } | null },
  Deleted: { name: 'annotation.deleted', schema: null as { paperID: string; highlightID: string; page: number } | null },
} as const;

/**
 * AI 处理相关事件
 */
export const AIEvents = {
  ThinkingStart: { name: 'ai.thinking.start', schema: null as { conversationID: string } | null },
  ThinkingEnd: { name: 'ai.thinking.end', schema: null as { conversationID: string } | null },
  StreamDelta: { name: 'ai.stream.delta', schema: null as { conversationID: string; delta: string } | null },
} as const;

/**
 * 合并所有事件定义
 */
export const JayreadBusEvents = {
  ...PaperEvents,
  ...AnnotationEvents,
  ...AIEvents,
} as const;

/**
 * 载荷类型
 */
export interface HighlightPayload {
  id: string;
  paper_id: string;
  page: number;
  text: string;
  color: string;
  rects: Array<{ x: number; y: number; w: number; h: number }>;
  created_at: string;
}

// ========== 事件总线 Store ==========

interface EventBusState {
  // 订阅者映射: eventName -> Set<listener>
  _subscribers: Map<string, Set<EventListener>>;
  // 历史记录（用于调试）
  _history: Array<{ event: string; payload: EventPayload; timestamp: number }>;
  // 调试模式
  _debug: boolean;
  // 监听器连续错误计数: listenerWeakRef -> consecutiveErrorCount
  _errorCounts: WeakMap<EventListener, number>;
}

// ========== 事件总线实现 ==========

/**
 * 创建事件总线
 */
function createEventBus() {
  // 内部状态
  const subscribers = new Map<string, Set<EventListener>>();
  const history: Array<{ event: string; payload: EventPayload; timestamp: number }> = [];
  const errorCounts = new WeakMap<EventListener, number>();

  /**
   * 发布事件
   */
  function emit<P extends EventPayload>(event: EventDefinition<P> | string, payload: P): void {
    const eventName = typeof event === 'string' ? event : event.name;
    const listeners = subscribers.get(eventName);

    if (listeners) {
      // 使用 Array.from 创建快照，避免在迭代时修改 Set
      const listenersArray = Array.from(listeners);
      for (const listener of listenersArray) {
        try {
          listener(payload);
          // 成功执行，重置错误计数
          errorCounts.delete(listener);
        } catch (error) {
          const currentCount = (errorCounts.get(listener) || 0) + 1;
          errorCounts.set(listener, currentCount);

          console.error(`[EventBus] Error in listener for "${eventName}":`, error);

          // 连续失败3次，自动移除该监听器
          if (currentCount >= 3) {
            listeners.delete(listener);
            console.warn(`[EventBus] Removed failing listener for "${eventName}" after ${currentCount} consecutive errors`);
          }
        }
      }
    }

    // 记录历史
    history.push({
      event: eventName,
      payload,
      timestamp: Date.now(),
    });

    // 限制历史记录数量
    if (history.length > 100) {
      history.shift();
    }

    if (process.env.NODE_ENV === 'development') {
      console.debug(`[EventBus] emit: ${eventName}`, payload);
    }
  }

  /**
   * 订阅事件
   */
  function on<P extends EventPayload>(
    event: EventDefinition<P> | string,
    listener: EventListener<P>
  ): Unsubscribe {
    const eventName = typeof event === 'string' ? event : event.name;

    if (!subscribers.has(eventName)) {
      subscribers.set(eventName, new Set());
    }

    const listeners = subscribers.get(eventName)!;

    // 添加订阅者数量限制，防止内存泄漏
    const MAX_SUBSCRIBERS = 100;
    if (listeners.size >= MAX_SUBSCRIBERS) {
      console.warn(`[EventBus] Event "${eventName}" has ${listeners.size} subscribers, possible memory leak. Maximum allowed is ${MAX_SUBSCRIBERS}.`);
    }

    listeners.add(listener as EventListener);

    if (process.env.NODE_ENV === 'development') {
      console.debug(`[EventBus] subscribe: ${eventName}`);
    }

    // 返回退订函数
    return () => {
      const set = subscribers.get(eventName);
      if (set) {
        set.delete(listener as EventListener);
        if (set.size === 0) {
          subscribers.delete(eventName);
        }
      }
      if (process.env.NODE_ENV === 'development') {
        console.debug(`[EventBus] unsubscribe: ${eventName}`);
      }
    };
  }

  /**
   * 订阅事件（仅触发一次）
   */
  function once<P extends EventPayload>(
    event: EventDefinition<P> | string,
    listener: EventListener<P>
  ): void {
    const eventName = typeof event === 'string' ? event : event.name;
    const wrappedListener = (payload: P) => {
      off(eventName, wrappedListener as EventListener);
      listener(payload);
    };
    on(eventName, wrappedListener as EventListener);
  }

  /**
   * 退订事件
   */
  function off<P extends EventPayload>(
    event: EventDefinition<P> | string,
    listener: EventListener<P>
  ): void {
    const eventName = typeof event === 'string' ? event : event.name;
    const set = subscribers.get(eventName);
    if (set) {
      set.delete(listener as EventListener);
      if (set.size === 0) {
        subscribers.delete(eventName);
      }
    }
  }

  /**
   * 获取事件订阅者数量
   */
  function listenerCount(eventName?: string): number {
    if (eventName) {
      return subscribers.get(eventName)?.size ?? 0;
    }
    let count = 0;
    subscribers.forEach(set => {
      count += set.size;
    });
    return count;
  }

  /**
   * 获取历史记录
   */
  function getHistory(): ReadonlyArray<{ event: string; payload: EventPayload; timestamp: number }> {
    return history;
  }

  /**
   * 清除历史记录
   */
  function clearHistory(): void {
    history.length = 0;
  }

  /**
   * 清除所有订阅者
   */
  function clearAll(): void {
    subscribers.clear();
  }

  return {
    emit,
    on,
    once,
    off,
    listenerCount,
    getHistory,
    clearHistory,
    clearAll,
  };
}

// ========== 单例导出 ==========

/**
 * Autonomics 事件总线单例
 *
 * 使用方式：
 * ```typescript
 * import { JayreadBus } from './bus';
 *
 * // 订阅事件
 * const unsubscribe = JayreadBus.on(JayreadBus.Events.PaperOpened, (payload) => {
 *   console.log('论文已打开:', payload.paperID);
 * });
 *
 * // 发布事件
 * JayreadBus.emit(JayreadBus.Events.PaperOpened, { paperID: '123' });
 *
 * // 退订
 * unsubscribe();
 * ```
 */
export const JayreadBus = createEventBus();

/**
 * 便捷的事件发射器
 *
 * @example
 * emit('paper.opened', { paperID: '123' });
 * emit(PaperEvents.Opened, { paperID: '123' });
 */
export const emit = JayreadBus.emit;
export const on = JayreadBus.on;
export const once = JayreadBus.once;
export const off = JayreadBus.off;

// ========== TransactionNotifier ==========

export { TransactionNotifier } from './transactionNotifier';
export type { RegisterObserverOptions } from './transactionNotifier';
export type {
  NotifierType,
  NotifierEvent,
  NotifierObserver,
  ExistenceChecker,
  EventQueue,
} from './types';

/**
 * 全局 TransactionNotifier 单例
 *
 * 自动桥接到 JayreadBus：notifier 处理后的事件以 `${type}.${event}` 格式转发到 bus，
 * 使 useBusSubscription 和 JayreadBus.on() 的订阅者都能收到通知。
 */
import { TransactionNotifier } from './transactionNotifier';
export const notifier = new TransactionNotifier({
  emit: (eventName, payload) => {
    JayreadBus.emit(eventName as any, payload as any);
  },
});

// ========== SSE Bridge ==========

export {
  bridgeParseDone,
  bridgeParseError,
  bridgePaperAdded,
  bridgePaperDeleted,
  bridgeFolderAdded,
  bridgeFolderModified,
  bridgeFolderDeleted,
} from './sseBridge';
