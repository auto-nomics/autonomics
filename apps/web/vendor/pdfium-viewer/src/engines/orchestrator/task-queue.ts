/**
 * @file 工作任务队列（WorkerTaskQueue）— 优先级调度与并发控制
 *
 * 整体作用：
 *   为 PDF 引擎提供带优先级的异步任务调度器。由于 PDFium 是单线程的
 *   （WASM 模块内部不可多线程并发），所有操作必须串行执行。本队列通过
 *   优先级排序确保高优先级任务（如可见页面的渲染）优先执行，低优先级
 *   任务（如后台搜索）后排。
 *
 * 核心设计决策：
 *   1. **延迟执行（Deferred Execution）**：入队时不立即调用执行函数，
 *      而是存储工厂函数（executeFactory），等到实际轮到该任务时才调用。
 *      这避免了提前创建 Task 对象导致的资源浪费。
 *   2. **优先级排序**：每次处理前对队列排序，先按 Priority 枚举值降序，
 *      再按 ranker 函数评分排序，最后按入队时间（FIFO）排序。
 *   3. **代理 Task 模式**：enqueue() 立即返回一个代理 Task，当实际执行
 *      时将代理 Task 与真实 Task 桥接（resolve/reject/progress 转发）。
 *   4. **可取消性**：通过代理 Task 的 abort() 方法，支持从队列中移除
 *      尚未开始执行的任务。
 */

import { Task, TaskError, Logger, NoopLogger } from '../../models';

/** 日志来源和分类标识 */
const LOG_SOURCE = 'TaskQueue';
const LOG_CATEGORY = 'Queue';

/**
 * 任务优先级枚举
 *
 * 数值越高优先级越大。各优先级的典型使用场景：
 *   - CRITICAL (3)：打开文档、关闭文档、当前可见页面的整页渲染
 *   - HIGH (2)：可见页面局部区域的渲染（renderPageRect）
 *   - MEDIUM (1)：元数据读写、注解操作、缩略图渲染等一般操作
 *   - LOW (0)：后台搜索、批量注解获取等非实时操作
 */
export enum Priority {
  CRITICAL = 3,
  HIGH = 2,
  MEDIUM = 1,
  LOW = 0,
}

// ============================================================================
// 类型工具 — 从 Task 泛型中提取结果、错误和进度的类型
// ============================================================================

/** 从 Task 类型中提取结果类型 R */
export type ExtractTaskResult<T> = T extends Task<infer R, any, any> ? R : never;

/** 从 Task 类型中提取错误类型 D */
export type ExtractTaskError<T> = T extends Task<any, infer D, any> ? D : never;

/** 从 Task 类型中提取进度类型 P */
export type ExtractTaskProgress<T> = T extends Task<any, any, infer P> ? P : never;

// ============================================================================
// 队列接口定义
// ============================================================================

/**
 * 队列中的任务项
 *
 * @property id             - 唯一标识符（UUID），用于取消和追踪
 * @property priority       - 任务优先级（CRITICAL / HIGH / MEDIUM / LOW）
 * @property meta           - 可选的元数据（如 docId、pageIndex、operation 等）
 * @property executeFactory - 延迟执行的工厂函数，仅在轮到执行时调用
 * @property cancelled      - 是否已被取消（用于跳过已取消的任务）
 */
export interface QueuedTask<T extends Task<any, any, any>> {
  id: string;
  priority: Priority;
  meta?: Record<string, unknown>;
  executeFactory: () => T; // 延迟执行：存储工厂函数，不在入队时调用！
  cancelled?: boolean;
}

/** 入队选项 */
export interface EnqueueOptions {
  /** 任务优先级，默认 MEDIUM */
  priority?: Priority;
  /** 任务元数据（用于日志和调试） */
  meta?: Record<string, unknown>;
  /** 是否强制 FIFO（先进先出）模式，跳过优先级排序 */
  fifo?: boolean;
}

/** 自定义任务比较器 — 完全控制排序逻辑 */
export type TaskComparator = (a: QueuedTask<any>, b: QueuedTask<any>) => number;

/** 任务评分函数 — 返回数值评分，分数越高越优先 */
export type TaskRanker = (task: QueuedTask<any>) => number;

/** 工作队列配置选项 */
export interface WorkerTaskQueueOptions {
  /** 最大并发数（默认 1，因为 PDFium 是单线程的） */
  concurrency?: number;
  /** 自定义排序比较器（优先级相同时使用） */
  comparator?: TaskComparator;
  /** 任务评分函数（用于同优先级内的细粒度排序） */
  ranker?: TaskRanker;
  /** 队列空闲时的回调 */
  onIdle?: () => void;
  /** 队列最大长度（防止内存溢出） */
  maxQueueSize?: number;
  /** 是否自动开始处理（默认 true） */
  autoStart?: boolean;
  /** 日志记录器 */
  logger?: Logger;
}

// ============================================================================
// WorkerTaskQueue — 带优先级的延迟执行任务队列
// ============================================================================

/**
 * 工作任务队列
 *
 * 核心机制：
 *   1. enqueue() 接收一个工厂函数，创建代理 Task 立即返回
 *   2. process() 循环中，对队列按优先级排序，取出最高优先级任务
 *   3. 此时才调用工厂函数创建真实 Task，并将结果桥接到代理 Task
 *   4. 真实 Task 完成后，处理下一个任务
 *
 * 为什么要延迟执行？
 *   如果在入队时就调用工厂函数，所有任务的 WASM 操作会立即开始竞争。
 *   延迟到实际执行时才创建 Task，可以确保 PDFium 单线程的串行执行语义。
 */
export class WorkerTaskQueue {
  /** 待执行的任务队列（按优先级排序） */
  private queue: QueuedTask<any>[] = [];
  /** 当前正在执行的任务数量 */
  private running = 0;
  /** 代理 Task 映射：id → resultTask，用于桥接真实 Task 的结果 */
  private resultTasks = new Map<string, Task<any, any, any>>();
  /** 日志记录器 */
  private logger: Logger;
  /** 队列配置 */
  private opts: Required<Omit<WorkerTaskQueueOptions, 'comparator' | 'ranker' | 'logger'>> & {
    comparator?: TaskComparator;
    ranker?: TaskRanker;
  };

  constructor(options: WorkerTaskQueueOptions = {}) {
    const {
      concurrency = 1,
      comparator,
      ranker,
      onIdle,
      maxQueueSize,
      autoStart = true,
      logger,
    } = options;
    this.logger = logger ?? new NoopLogger();
    this.opts = {
      concurrency: Math.max(1, concurrency), // 并发数至少为 1
      comparator,
      ranker,
      onIdle: onIdle ?? (() => {}),
      maxQueueSize: maxQueueSize ?? Number.POSITIVE_INFINITY,
      autoStart,
    };
  }

  /** 动态设置自定义比较器 */
  setComparator(comparator?: TaskComparator): void {
    this.opts.comparator = comparator;
  }

  /** 动态设置任务评分函数 */
  setRanker(ranker?: TaskRanker): void {
    this.opts.ranker = ranker;
  }

  /** 当前队列中等待执行的任务数 */
  size(): number {
    return this.queue.length;
  }

  /** 当前正在执行的任务数 */
  inFlight(): number {
    return this.running;
  }

  /** 队列是否空闲（无等待任务且无执行中任务） */
  isIdle(): boolean {
    return this.queue.length === 0 && this.running === 0;
  }

  /**
   * 等待队列完全排空（所有任务执行完毕）
   *
   * 通过注册一次性 idle 监听器实现，当队列为空时自动 resolve。
   */
  async drain(): Promise<void> {
    if (this.isIdle()) return;
    await new Promise<void>((resolve) => {
      const check = () => {
        if (this.isIdle()) {
          this.offIdle(check);
          resolve();
        }
      };
      this.onIdle(check);
    });
  }

  // ── 空闲监听器机制 ──

  private idleListeners = new Set<() => void>();

  /** 通知所有空闲监听器，并触发 onIdle 回调 */
  private notifyIdle() {
    if (this.isIdle()) {
      [...this.idleListeners].forEach((fn) => fn());
      this.idleListeners.clear();
      this.opts.onIdle();
    }
  }

  private onIdle(fn: () => void) {
    this.idleListeners.add(fn);
  }

  private offIdle(fn: () => void) {
    this.idleListeners.delete(fn);
  }

  /**
   * 入队一个任务 — 使用延迟执行工厂模式
   *
   * 核心流程：
   *   1. 生成唯一 ID，确定优先级
   *   2. 创建代理 Task（resultTask）立即返回给调用者
   *   3. 将工厂函数存入队列，不立即执行
   *   4. 覆盖代理 Task 的 abort()，支持从队列中取消
   *   5. 如果 autoStart，触发 process() 开始调度
   *
   * @param taskDef.execute - 工厂函数，返回真实 Task（延迟调用！）
   * @param taskDef.meta    - 任务元数据
   * @param options         - 入队选项（优先级、元数据、FIFO 模式）
   * @returns 代理 Task，类型与工厂函数返回的 Task 完全相同
   *
   * @example
   * // 典型用法：高优先级渲染任务
   * const task = queue.enqueue({
   *   execute: () => executor.renderPageRaw(doc, page, options),
   *   meta: { docId: doc.id, pageIndex: page.index }
   * }, { priority: Priority.CRITICAL });
   */
  enqueue<T extends Task<any, any, any>>(
    taskDef: {
      execute: () => T; // 工厂函数 — 在轮到执行时才调用！
      meta?: Record<string, unknown>;
    },
    options: EnqueueOptions = {},
  ): T {
    const id = this.generateId();
    const priority = options.priority ?? Priority.MEDIUM;

    // 创建代理 Task：类型与工厂函数返回的 Task 保持一致
    // 调用者拿到的是这个代理 Task，真实 Task 稍后才会被创建
    const resultTask = new Task<
      ExtractTaskResult<T>,
      ExtractTaskError<T>,
      ExtractTaskProgress<T>
    >() as T;

    // 检查队列是否已满
    if (this.queue.length >= this.opts.maxQueueSize) {
      const error = new Error('Queue is full (maxQueueSize reached).');
      resultTask.reject(error as any);
      return resultTask;
    }

    // 将代理 Task 存入映射，等待后续桥接
    this.resultTasks.set(id, resultTask);

    // 构造队列任务项（存储工厂函数，不调用！）
    const queuedTask: QueuedTask<T> = {
      id,
      priority,
      meta: options.meta ?? taskDef.meta,
      executeFactory: taskDef.execute, // 仅存储，不调用
    };

    this.queue.push(queuedTask);

    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      `Task enqueued: ${id} | Priority: ${priority} | Running: ${this.running} | Queued: ${this.queue.length}`,
    );

    // 覆盖代理 Task 的 abort 方法：取消时从队列中移除
    const originalAbort = resultTask.abort.bind(resultTask);
    resultTask.abort = (reason: any) => {
      this.logger.debug(LOG_SOURCE, LOG_CATEGORY, `Task aborted: ${id}`);
      this.cancel(id);
      originalAbort(reason);
    };

    // 自动开始处理（如果配置了 autoStart）
    if (this.opts.autoStart) this.process(options.fifo === true);

    return resultTask;
  }

  /**
   * 从队列中取消并移除指定任务
   *
   * @param taskId - 要取消的任务 ID
   */
  private cancel(taskId: string): void {
    const before = this.queue.length;
    this.queue = this.queue.filter((t) => {
      if (t.id === taskId) {
        t.cancelled = true; // 标记为已取消
        return false; // 从队列中移除
      }
      return true;
    });

    this.resultTasks.delete(taskId);

    if (before !== this.queue.length) {
      this.logger.debug(LOG_SOURCE, LOG_CATEGORY, `Task cancelled and removed: ${taskId}`);
      this.kick(); // 触发下一轮调度
    }
  }

  /** 通过微任务触发下一轮调度，避免阻塞当前执行 */
  private kick() {
    queueMicrotask(() => this.process());
  }

  /**
   * 核心调度循环 — 从队列中取出任务并执行
   *
   * 循环条件：运行中任务数 < 并发上限 且 队列非空
   * 每次迭代：
   *   1. 按优先级排序队列
   *   2. 取出最高优先级任务
   *   3. 调用工厂函数创建真实 Task
   *   4. 将真实 Task 的结果桥接到代理 Task
   *   5. 等待 Task 完成后递减 running 计数
   *
   * @param fifo - 是否强制 FIFO 模式（跳过排序）
   */
  private async process(fifo = false): Promise<void> {
    this.logger.debug(
      LOG_SOURCE,
      LOG_CATEGORY,
      `process() called | Running: ${this.running} | Concurrency: ${this.opts.concurrency} | Queued: ${this.queue.length}`,
    );

    while (this.running < this.opts.concurrency && this.queue.length > 0) {
      this.logger.debug(
        LOG_SOURCE,
        LOG_CATEGORY,
        `Starting new task | Running: ${this.running} | Queued: ${this.queue.length}`,
      );

      // 非 FIFO 模式下，按优先级排序
      if (!fifo) this.sortQueue();

      // 取出队首任务（优先级最高）
      const queuedTask = this.queue.shift()!;
      if (queuedTask.cancelled) {
        // 跳过已取消的任务
        this.logger.debug(LOG_SOURCE, LOG_CATEGORY, `Skipping cancelled task: ${queuedTask.id}`);
        continue;
      }

      // 获取对应的代理 Task
      const resultTask = this.resultTasks.get(queuedTask.id);
      if (!resultTask) continue; // 防御性检查

      this.running++;

      // 异步执行任务，不阻塞调度循环
      (async () => {
        let realTask: Task<any, any, any> | null = null;

        try {
          // ★ 关键：此时才调用工厂函数，创建真实 Task！
          realTask = queuedTask.executeFactory();

          // 防御：工厂函数可能返回 null/undefined
          if (!realTask) {
            throw new Error('Task factory returned null/undefined');
          }

          // 将真实 Task 的结果桥接到代理 Task
          realTask.wait(
            (result) => {
              // 仅当代理 Task 尚未 resolve/reject 时才转发
              if (resultTask.state.stage === 0 /* Pending */) {
                resultTask.resolve(result);
              }
            },
            (error) => {
              if (resultTask.state.stage === 0 /* Pending */) {
                if (error.type === 'abort') {
                  resultTask.abort(error.reason);
                } else {
                  resultTask.reject(error.reason);
                }
              }
            },
          );

          // 桥接进度事件
          realTask.onProgress((progress) => {
            resultTask.progress(progress);
          });

          // 等待真实 Task 完成
          await realTask.toPromise();
        } catch (error) {
          // 捕获工厂函数或执行过程中的异常
          if (resultTask.state.stage === 0 /* Pending */) {
            resultTask.reject(error as any);
          }
        } finally {
          // 清理：移除代理 Task 映射，递减运行计数
          this.resultTasks.delete(queuedTask.id);
          this.running--;

          this.logger.debug(
            LOG_SOURCE,
            LOG_CATEGORY,
            `Task completed: ${queuedTask.id} | Running: ${this.running} | Queued: ${this.queue.length}`,
          );

          // 检查是否需要继续调度
          if (this.isIdle()) {
            this.notifyIdle();
          } else if (this.queue.length > 0) {
            this.kick();
          }
        }
      })().catch((error) => {
        // 兜底：捕获异步执行包装器中未处理的异常
        this.logger.error(
          LOG_SOURCE,
          LOG_CATEGORY,
          'Unhandled error in task execution wrapper:',
          error,
        );
        this.running = Math.max(0, this.running - 1);
        if (this.isIdle()) {
          this.notifyIdle();
        } else if (this.queue.length > 0) {
          this.kick();
        }
      });
    }
  }

  /**
   * 按优先级排序队列
   *
   * 排序规则（优先级从高到低）：
   *   1. Priority 枚举值降序（CRITICAL > HIGH > MEDIUM > LOW）
   *   2. ranker 评分降序（同优先级内按评分排序）
   *   3. 入队时间升序（同优先级同评分时，先进先出）
   *
   * 使用 rankCache 缓存评分结果，避免对同一任务重复调用 ranker。
   */
  private sortQueue(): void {
    const { comparator, ranker } = this.opts;

    // 如果提供了自定义比较器，直接使用
    if (comparator) {
      this.queue.sort(comparator);
      return;
    }

    // 默认排序：优先级 > ranker 评分 > 入队时间
    const rankCache = new Map<string, number>();
    const getRank = (t: QueuedTask<any>) => {
      if (!ranker) return this.defaultRank(t);
      if (!rankCache.has(t.id)) rankCache.set(t.id, ranker(t));
      return rankCache.get(t.id)!;
    };

    this.queue.sort((a, b) => {
      // 第一排序键：优先级（数值越大越优先）
      if (a.priority !== b.priority) return b.priority - a.priority;
      // 第二排序键：ranker 评分（数值越大越优先）
      const ar = getRank(a);
      const br = getRank(b);
      if (ar !== br) return br - ar;
      // 第三排序键：入队时间（越早越优先，FIFO）
      return this.extractTime(a.id) - this.extractTime(b.id);
    });
  }

  /** 默认评分（不额外排序，返回 0） */
  private defaultRank(_task: QueuedTask<any>): number {
    return 0;
  }

  /**
   * 生成唯一任务 ID
   *
   * 优先使用 crypto.randomUUID()（安全、唯一），
   * 在不支持的环境中降级为时间戳 + 随机字符串。
   */
  private generateId(): string {
    if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
      return crypto.randomUUID();
    }
    return `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  }

  /**
   * 从任务 ID 中提取时间戳（用于 FIFO 排序）
   *
   * ID 格式为 `timestamp-random` 或 UUID，
   * 此方法提取时间戳部分作为排序依据。
   */
  private extractTime(id: string): number {
    const t = Number(id.split('-')[0]);
    return Number.isFinite(t) ? t : 0;
  }
}
