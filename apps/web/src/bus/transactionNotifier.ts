/**
 * TransactionNotifier —— 事务感知的通知器
 *
 * 参考 Zotero Notifier 设计，提供：
 * 1. 事务批量：begin/commit 包裹，所有 trigger 入队，commit 时统一处理
 * 2. 去重合并：同一事务内对同一对象的多次修改合并为一次通知
 * 3. 无效事件过滤：被删除对象的 modify 事件被静默丢弃
 * 4. 确定性排序：类型顺序 + 事件顺序双重排序
 * 5. 容错：观察者回调用 try/catch 包装，一个观察者抛异常不影响其他
 *
 * @example
 * ```ts
 * import { notifier } from '@/bus';
 *
 * // 注册观察者
 * const obsId = notifier.registerObserver({
 *   notify(event, type, ids, data) {
 *     console.log(event, type, ids);
 *   }
 * }, ['paper', 'annotation']);
 *
 * // 事务批量操作
 * const txId = notifier.begin();
 * notifier.trigger('add', 'paper', ['p1', 'p2']);
 * notifier.trigger('modify', 'paper', ['p1']);
 * await notifier.commit(txId);
 *
 * // 清理
 * notifier.unregisterObserver(obsId);
 * ```
 */

import type {
  NotifierType,
  NotifierEvent,
  EventQueue,
  NotifierObserver,
  ObserverRecord,
  ExistenceChecker,
} from './types';
import { VALID_TYPES, VALID_EVENTS } from './types';
import { sortTypes, sortEvents } from './ordering';
import { mergeEvent, compressEvents, deduplicateIds, filterInvalidEvents } from './dedup';

/** 观察者注册选项 */
export interface RegisterObserverOptions {
  /** 只监听这些类型；null/undefined = 监听所有 */
  types?: NotifierType[] | null;
  /** 执行优先级，数值越小越先执行。默认 100 */
  priority?: number;
  /** 自定义 ID（用于调试），不传则自动生成 */
  id?: string;
}

let _observerIdCounter = 0;

function generateObserverId(hint?: string): string {
  _observerIdCounter++;
  const rand = Math.random().toString(36).slice(2, 6);
  return hint ? `${hint}_${rand}` : `obs_${rand}_${_observerIdCounter}`;
}

export class TransactionNotifier {
  /** 当前事务 ID（null = 不在事务中） */
  private _transactionID: string | null = null;
  /** 当前事务的事件队列 */
  private _queue: EventQueue = {};
  /** 已注册的观察者 */
  private _observers = new Map<string, ObserverRecord>();
  /** 对象存在性检查器（可选，通过 setExistenceChecker 注入） */
  private _existenceChecker?: ExistenceChecker;
  /** 外部事件总线（可选，用于桥接到 JayreadBus） */
  private _bus?: { emit: (eventName: string, payload: Record<string, unknown>) => void };
  /** bridge observer 的注册 ID */
  private _bridgeObserverId?: string;

  /**
   * @param bus 可选的外部事件总线（如 JayreadBus），注入后自动注册 bridge observer
   */
  constructor(bus?: { emit: (eventName: string, payload: Record<string, unknown>) => void }) {
    this._bus = bus;
    if (this._bus) {
      this._setupBridge();
    }
  }

  // ========== 事务控制 ==========

  /**
   * 开始事务。后续所有 trigger() 调用将入队而非直接通知。
   * @param transactionId 可选的事务 ID，不传则自动生成
   * @returns 事务 ID
   */
  begin(transactionId?: string): string {
    const id = transactionId ?? `tx_${Date.now()}_${Math.random().toString(36).slice(2, 6)}`;
    this._transactionID = id;
    this._queue = {};
    return id;
  }

  /**
   * 提交事务：对队列中的事件执行去重 → 压缩 → 过滤 → 排序 → 通知。
   * @param transactionId 可选，传入时校验是否匹配当前事务
   */
  async commit(transactionId?: string): Promise<void> {
    if (transactionId && transactionId !== this._transactionID) {
      console.warn(
        `[TransactionNotifier] commit() called with mismatched ID: ${transactionId} vs ${this._transactionID}`,
      );
      return;
    }

    if (!this._transactionID) {
      throw new Error('[TransactionNotifier] commit() called outside of a transaction');
    }

    // 取出队列快照并重置
    const queue = this._queue;
    this._queue = {};
    this._transactionID = null;

    // 1. 压缩事件（去重 + 合并规则）
    let processed = compressEvents(queue);

    // 2. 去重 ids
    deduplicateIds(processed);

    // 3. 过滤无效事件（如果有 existenceChecker）
    processed = await filterInvalidEvents(processed, this._existenceChecker);

    // 4. 按确定性顺序通知
    await this._dispatchQueue(processed);
  }

  /**
   * 重置（回滚）：清空队列并结束事务。
   */
  reset(transactionId?: string): void {
    if (transactionId && transactionId !== this._transactionID) {
      return;
    }
    this._queue = {};
    this._transactionID = null;
  }

  // ========== 事件触发 ==========

  /**
   * 触发事件。
   * - 事务内：入队（同步，返回 void）
   * - 事务外：直接通知观察者（异步，返回 Promise<void>）
   */
  trigger(
    event: NotifierEvent,
    type: NotifierType,
    ids: string[],
    extraData?: Record<string, unknown>,
  ): Promise<void> {
    this._validateType(type);
    this._validateEvent(event);

    if (this._transactionID) {
      mergeEvent(this._queue, event, type, ids, extraData);
      return Promise.resolve();
    }

    // 事务外：异步通知观察者
    return this._notifyObservers(event, type, ids, extraData ?? {});
  }

  // ========== 观察者管理 ==========

  /**
   * 注册观察者。
   * @returns 观察者 ID（用于后续注销）
   */
  registerObserver(
    observer: NotifierObserver,
    types?: NotifierType[] | null,
    priority?: number,
  ): string;
  registerObserver(observer: NotifierObserver, options?: RegisterObserverOptions): string;
  registerObserver(
    observer: NotifierObserver,
    typesOrOptions?: NotifierType[] | RegisterObserverOptions | null,
    priority?: number,
  ): string {
    let types: NotifierType[] | null = null;
    let prio = 100;
    let hint: string | undefined;

    if (Array.isArray(typesOrOptions)) {
      types = typesOrOptions;
      prio = priority ?? 100;
    } else if (typesOrOptions && typeof typesOrOptions === 'object') {
      types = typesOrOptions.types ?? null;
      prio = typesOrOptions.priority ?? 100;
      hint = typesOrOptions.id;
    }

    if (types) {
      for (const t of types) {
        this._validateType(t);
      }
    }

    const id = generateObserverId(hint);
    this._observers.set(id, { ref: observer, types, priority: prio });
    return id;
  }

  /**
   * 注销观察者。
   */
  unregisterObserver(observerId: string): void {
    this._observers.delete(observerId);
  }

  /**
   * 设置存在性检查器（用于过滤已删除对象的 modify 事件）。
   */
  setExistenceChecker(checker: ExistenceChecker | undefined): void {
    this._existenceChecker = checker;
  }

  /** 当前是否处于事务中 */
  get inTransaction(): boolean {
    return this._transactionID !== null;
  }

  /** 当前注册的观察者数量 */
  get observerCount(): number {
    return this._observers.size;
  }

  /** 获取调试快照（不暴露内部可变引用） */
  getDebugInfo(): {
    activeTransactionId: string | null;
    observerCount: number;
    observers: Array<{ id: string; types: NotifierType[] | null; priority: number }>;
    hasBus: boolean;
  } {
    const observers: Array<{ id: string; types: NotifierType[] | null; priority: number }> = [];
    for (const [id, record] of this._observers) {
      observers.push({ id, types: record.types, priority: record.priority });
    }
    return {
      activeTransactionId: this._transactionID,
      observerCount: this._observers.size,
      observers,
      hasBus: !!this._bus,
    };
  }

  // ========== 内部方法 ==========

  /**
   * 按确定性顺序分发队列中的所有事件。
   */
  private async _dispatchQueue(queue: EventQueue): Promise<void> {
    const types = sortTypes(Object.keys(queue) as NotifierType[]);

    for (const type of types) {
      const typeQueue = queue[type];
      if (!typeQueue) continue;

      const events = sortEvents(Object.keys(typeQueue) as NotifierEvent[]);

      for (const event of events) {
        const entry = typeQueue[event];
        if (!entry || entry.ids.length === 0) continue;

        await this._notifyObservers(event, type, entry.ids, entry.data);
      }
    }
  }

  /**
   * 通知匹配的观察者（带优先级排序和容错）。
   */
  private async _notifyObservers(
    event: NotifierEvent,
    type: NotifierType,
    ids: string[],
    extraData: Record<string, unknown>,
  ): Promise<void> {
    const ordered = this._getObserverOrder(type);

    for (const { ref } of ordered) {
      try {
        await Promise.resolve(ref.notify(event, type, ids, extraData));
      } catch (e) {
        console.error(
          `[TransactionNotifier] Observer error for ${type}/${event}:`,
          e,
        );
      }
    }
  }

  /**
   * 获取监听指定类型的观察者，按优先级排序。
   * 优先级数值越小越先执行；无优先级默认排最后。
   */
  private _getObserverOrder(
    type: NotifierType,
  ): ObserverRecord[] {
    const matched: (ObserverRecord & { id: string })[] = [];

    for (const [id, record] of this._observers) {
      // null types = 监听所有类型
      if (!record.types || record.types.includes(type)) {
        matched.push({ ...record, id });
      }
    }

    matched.sort((a, b) => {
      return a.priority - b.priority;
    });

    return matched;
  }

  private _validateType(type: string): void {
    if (!VALID_TYPES.includes(type as NotifierType)) {
      throw new Error(`[TransactionNotifier] Invalid type: '${type}'`);
    }
  }

  private _validateEvent(event: string): void {
    if (!VALID_EVENTS.includes(event as NotifierEvent)) {
      throw new Error(`[TransactionNotifier] Invalid event: '${event}'`);
    }
  }

  /**
   * 注册 bridge observer —— 将 notifier 事件转发到 JayreadBus。
   * 使用最低优先级 (999)，确保所有业务观察者先处理完毕。
   */
  private _setupBridge(): void {
    if (!this._bus) return;
    const bus = this._bus;

    this._bridgeObserverId = this.registerObserver({
      notify(event, type, ids, extraData) {
        bus.emit(`${type}.${event}`, { ids, ...extraData });
      },
    }, { priority: 999, id: 'bus-bridge' });
  }
}
