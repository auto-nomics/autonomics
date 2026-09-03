/**
 * @file cache.ts
 * @description PDF 页面缓存管理系统
 *
 * 本文件实现了一套多层级的 PDF 页面缓存系统，用于管理 PDFium 中的文档、
 * 页面加载和生命周期。系统采用引用计数（RC）+ LRU（最近最少使用）+
 * TTL（生存时间）三种策略的组合来管理缓存。
 *
 * 架构层级（从外到内）：
 *   PdfCache（顶层缓存管理器）
 *     └─ DocumentContext（文档上下文，每个打开的 PDF 文件一个）
 *          └─ PageCache（页面缓存，管理单个文档的所有页面）
 *               └─ PageContext（页面上下文，单个 PDF 页面的生命周期管理）
 *
 * 缓存策略：
 *   - 引用计数（RC）：追踪页面是否正在被使用，正在使用的页面不会被驱逐
 *   - LRU：当缓存超过容量上限时，优先驱逐最久未访问的页面
 *   - TTL：页面释放后不会立即销毁，而是等待 TTL 超时后再清理，
 *          避免短时间内反复加载同一页面
 *
 * 线程安全说明：
 *   由于 PDFium 的单实例限制（参见 feedback_pdfium_thread_safe），
 *   此缓存系统运行在 Web Worker 中，通过消息传递与主线程通信。
 */

import { WrappedPdfiumModule } from '../../pdfium';
import { MemoryManager } from './core/memory-manager';
import { WasmPointer } from './types/branded';

/**
 * 缓存配置接口
 */
export interface CacheConfig {
  /**
   * 页面在缓存中的生存时间（毫秒）
   * 当页面的引用计数降为 0 后，会启动一个定时器，超时后自动清理页面资源。
   * 默认值：5000ms（5 秒）
   */
  pageTtl?: number;
  /**
   * 每个文档在缓存中保留的最大页面数
   * 超过此数量时，会按 LRU 策略驱逐最久未使用的页面。
   * 默认值：10 页
   */
  maxPagesPerDocument?: number;
  /**
   * 是否使用标准化旋转加载页面
   * 当为 true 时：
   *   - 所有坐标（注释、文本、渲染）都在 0° 旋转空间中
   *   - 原始旋转角度被保留供参考
   * 默认值：false
   */
  normalizeRotation?: boolean;
}

/** 默认缓存配置 */
const DEFAULT_CONFIG: Required<CacheConfig> = {
  pageTtl: 5000, // 5 秒生存时间
  maxPagesPerDocument: 10, // 每个文档最多缓存 10 页
  normalizeRotation: false,
};

/**
 * LRU 双向链表节点
 *
 * 用于 O(1) 时间复杂度的页面访问顺序更新。
 */
interface LRUNode {
  /** 页面索引 */
  pageIdx: number;
  /** 前驱节点（更接近头节点，即更久未使用） */
  prev: LRUNode | null;
  /** 后继节点（更接近尾节点，即更最近使用） */
  next: LRUNode | null;
}

/**
 * 顶层 PDF 缓存管理器
 *
 * 管理所有打开的 PDF 文档的缓存。每个文档由一个 DocumentContext 表示，
 * 包含该文档的所有页面缓存和相关的 PDFium 资源。
 *
 * 职责：
 *   - 文档的打开/关闭/查找
 *   - 全局缓存配置管理
 *   - 缓存统计信息收集
 */
export class PdfCache {
  /** 文档 ID → 文档上下文的映射表 */
  private readonly docs = new Map<string, DocumentContext>();
  /** 当前生效的缓存配置（所有文档共享） */
  private readonly config: Required<CacheConfig>;

  /**
   * @param pdfium - PDFium WASM 模块的封装实例，提供底层 PDF 操作 API
   * @param memoryManager - WASM 内存管理器，用于正确追踪和释放文件数据的内存
   * @param config - 可选的缓存配置，未指定的项使用默认值
   */
  constructor(
    private readonly pdfium: WrappedPdfiumModule,
    private readonly memoryManager: MemoryManager,
    config: CacheConfig = {},
  ) {
    this.config = { ...DEFAULT_CONFIG, ...config };
  }

  /**
   * 注册（或复用）一个已打开的 PDF 文档
   *
   * 如果指定 ID 的文档已存在，则不做任何操作（复用现有上下文）。
   * 否则创建新的 DocumentContext 并存入映射表。
   *
   * @param id - 文档的唯一标识符
   * @param filePtr - 文件数据在 WASM 堆中的指针
   * @param docPtr - PDFium 文档句柄指针
   * @param normalizeRotation - 是否使用标准化旋转（覆盖全局配置）
   */
  setDocument(id: string, filePtr: number, docPtr: number, normalizeRotation: boolean = false) {
    let ctx = this.docs.get(id);
    if (!ctx) {
      // 为新文档创建上下文，使用文档级别的 normalizeRotation 覆盖全局配置
      const docConfig = { ...this.config, normalizeRotation };
      ctx = new DocumentContext(filePtr, docPtr, this.pdfium, this.memoryManager, docConfig);
      this.docs.set(id, ctx);
    }
  }

  /**
   * 获取指定文档的上下文
   *
   * @param docId - 文档的唯一标识符
   * @returns 文档上下文实例，如果文档未打开则返回 undefined
   */
  getContext(docId: string): DocumentContext | undefined {
    return this.docs.get(docId);
  }

  /**
   * 关闭并完全释放一个文档及其所有页面
   *
   * @param docId - 要关闭的文档 ID
   * @returns true 表示成功关闭，false 表示文档不存在
   */
  closeDocument(docId: string): boolean {
    const ctx = this.docs.get(docId);
    if (!ctx) return false;
    this.docs.delete(docId);
    ctx.dispose();
    return true;
  }

  /**
   * 关闭所有已打开的文档，释放所有资源
   */
  closeAllDocuments(): void {
    for (const ctx of this.docs.values()) {
      ctx.dispose();
    }
    this.docs.clear();
  }

  /**
   * 更新所有已存在文档的缓存配置
   *
   * 配置变更会立即生效：
   *   - TTL 变更会影响所有已缓存的页面
   *   - maxPagesPerDocument 变更可能触发页面驱逐
   *
   * @param newConfig - 新的缓存配置（部分指定即可，未指定的保持不变）
   */
  updateConfig(newConfig: CacheConfig): void {
    Object.assign(this.config, newConfig);
    // 将配置变更传播到所有已打开的文档
    for (const ctx of this.docs.values()) {
      ctx.updateConfig(this.config);
    }
  }

  /**
   * 获取当前缓存统计信息
   *
   * 用于监控和调试缓存使用情况。
   *
   * @returns 包含以下字段的统计对象：
   *   - documents：当前打开的文档数量
   *   - totalPages：所有文档中缓存的页面总数
   *   - pagesByDocument：每个文档的缓存页面数（文档 ID → 页数）
   */
  getCacheStats(): {
    documents: number;
    totalPages: number;
    pagesByDocument: Record<string, number>;
  } {
    const pagesByDocument: Record<string, number> = {};
    let totalPages = 0;

    for (const [docId, ctx] of this.docs.entries()) {
      const pageCount = ctx.getCacheSize();
      pagesByDocument[docId] = pageCount;
      totalPages += pageCount;
    }

    return {
      documents: this.docs.size,
      totalPages,
      pagesByDocument,
    };
  }
}

/**
 * 文档上下文
 *
 * 表示一个已打开的 PDF 文档，管理其页面缓存和 PDFium 资源。
 * 每个文档拥有独立的 PageCache 实例。
 *
 * 生命周期：
 *   创建（setDocument）→ 使用（acquirePage/borrowPage）→ 销毁（dispose/closeDocument）
 *
 * 资源管理：
 *   dispose 时按顺序释放：
 *   1. 所有缓存的页面（包括它们的文本页和注释资源）
 *   2. PDFium 文档句柄（FPDF_CloseDocument）
 *   3. WASM 堆中的文件数据（通过 MemoryManager）
 */
export class DocumentContext {
  /** 该文档的页面缓存实例 */
  private readonly pageCache: PageCache;
  /** 是否使用标准化旋转模式 */
  public readonly normalizeRotation: boolean;
  /** 防止重复销毁的标志 */
  private disposed = false;

  /**
   * @param filePtr - 文件数据在 WASM 堆中的指针（用于最终释放）
   * @param docPtr - PDFium 文档句柄指针（FPDF_Document）
   * @param pdfium - PDFium WASM 模块封装
   * @param memoryManager - WASM 内存管理器
   * @param config - 完整的缓存配置
   */
  constructor(
    public readonly filePtr: number,
    public readonly docPtr: number,
    pdfium: WrappedPdfiumModule,
    private readonly memoryManager: MemoryManager,
    config: Required<CacheConfig>,
  ) {
    this.normalizeRotation = config.normalizeRotation;
    this.pageCache = new PageCache(pdfium, docPtr, config);
  }

  /**
   * 获取（或加载）指定页面的上下文
   *
   * 这是获取页面的主要方法。返回的 PageContext 引用计数已增加，
   * 使用完毕后必须调用 release() 或 disposeImmediate()。
   *
   * @param pageIdx - 页面索引（从 0 开始）
   * @returns 页面上下文实例
   */
  acquirePage(pageIdx: number): PageContext {
    return this.pageCache.acquire(pageIdx);
  }

  /**
   * 借用页面执行一次性操作
   *
   * 提供了一种安全的"借用"模式：
   *   - 如果页面已在缓存中 → 操作后调用 release()（保持 TTL 逻辑）
   *   - 如果页面是新加载的 → 操作后立即释放（不保留在缓存中）
   *
   * 使用 try/finally 确保无论操作是否抛出异常，页面资源都会被正确释放。
   *
   * @param pageIdx - 页面索引
   * @param fn - 要在页面上下文中执行的函数
   * @returns fn 的返回值
   */
  borrowPage<T>(pageIdx: number, fn: (ctx: PageContext) => T): T {
    return this.pageCache.borrowPage(pageIdx, fn);
  }

  /**
   * 更新页面缓存配置
   * @param config - 新的缓存配置
   */
  updateConfig(config: Required<CacheConfig>): void {
    this.pageCache.updateConfig(config);
  }

  /**
   * 获取当前缓存中的页面数量
   */
  getCacheSize(): number {
    return this.pageCache.size();
  }

  /**
   * 销毁文档上下文，释放所有资源
   *
   * 释放顺序（顺序很重要，避免悬空指针）：
   *   1. 释放所有缓存的页面（包括文本页、注释等子资源）
   *   2. 关闭 PDFium 文档句柄
   *   3. 通过 MemoryManager 释放 WASM 堆中的文件数据
   *
   * 使用 disposed 标志防止重复销毁。
   */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;

    // 1. 释放所有缓存的页面（立即释放，不等待 TTL）
    this.pageCache.forceReleaseAll();

    // 2. 关闭 PDFium 文档句柄
    this.pageCache.pdf.FPDF_CloseDocument(this.docPtr);

    // 3. 通过内存管理器释放文件数据（确保内存追踪的准确性）
    this.memoryManager.free(WasmPointer(this.filePtr));
  }
}

/**
 * 页面缓存
 *
 * 管理单个 PDF 文档中所有页面的缓存。实现了 LRU（最近最少使用）驱逐策略
 * 和基于引用计数 + TTL 的生命周期管理。
 *
 * 缓存流程：
 *   acquire(pageIdx)：
 *     → 缓存命中：更新 LRU 顺序，增加引用计数，清除 TTL 定时器
 *     → 缓存未命中：检查是否需要驱逐 → 加载页面 → 创建 PageContext → 更新 LRU
 *
 * 驱逐策略（LRU）：
 *   - accessOrder 数组记录页面的访问顺序（数组头部=最久未使用，尾部=最近使用）
 *   - 当缓存大小超过 maxPagesPerDocument 时，从数组头部开始驱逐
 *   - 只驱逐引用计数为 0 的页面（正在使用的页面不会被驱逐）
 */
export class PageCache {
  /** 页面索引 → 页面上下文的映射表 */
  private readonly cache = new Map<number, PageContext>();
  /** LRU 双向链表的头节点（最久未使用） */
  private lruHead: LRUNode | null = null;
  /** LRU 双向链表的尾节点（最近使用） */
  private lruTail: LRUNode | null = null;
  /** 页面索引 → LRU 节点的映射表，提供 O(1) 访问 */
  private readonly lruMap = new Map<number, LRUNode>();
  /** 当前生效的缓存配置 */
  private config: Required<CacheConfig>;

  /**
   * @param pdf - PDFium WASM 模块封装
   * @param docPtr - 所属文档的 PDFium 句柄
   * @param config - 缓存配置
   */
  constructor(
    public readonly pdf: WrappedPdfiumModule,
    private readonly docPtr: number,
    config: Required<CacheConfig>,
  ) {
    this.config = config;
  }

  /**
   * 获取（或加载）指定页面
   *
   * 此方法是页面获取的核心。它处理缓存命中和未命中的情况，
   * 并自动管理 LRU 顺序和引用计数。
   *
   * @param pageIdx - 页面索引（从 0 开始）
   * @returns 页面上下文实例（引用计数已增加）
   */
  acquire(pageIdx: number): PageContext {
    let ctx = this.cache.get(pageIdx);

    if (!ctx) {
      // 缓存未命中：检查是否需要驱逐后再加载
      this.evictIfNeeded();

      let pagePtr: number;
      if (this.config.normalizeRotation) {
        // 使用标准化旋转模式加载页面
        // 所有坐标将处于 0° 旋转空间中，原始旋转信息保存在 PdfPageObject 中
        // 传入 0（空指针）因为旋转信息已在其他地方存储
        pagePtr = this.pdf.EPDF_LoadPageNormalized(this.docPtr, pageIdx, 0);
      } else {
        // 使用标准模式加载页面（保留原始旋转）
        pagePtr = this.pdf.FPDF_LoadPage(this.docPtr, pageIdx);
      }

      // 创建页面上下文，并传入清理回调
      // onFinalDispose 回调在页面被最终销毁时调用，负责从缓存和 LRU 数组中移除
      ctx = new PageContext(this.pdf, this.docPtr, pageIdx, pagePtr, this.config.pageTtl, () => {
        this.cache.delete(pageIdx);
        this.removeFromAccessOrder(pageIdx);
      });
      this.cache.set(pageIdx, ctx);
    }

    // 更新 LRU 访问顺序（将当前页面移到数组末尾）
    this.updateAccessOrder(pageIdx);

    // 清除可能存在的 TTL 定时器（页面正在被使用，不应过期）
    ctx.clearExpiryTimer();
    // 增加引用计数，标记页面正在被使用
    ctx.bumpRefCount();
    return ctx;
  }

  /**
   * 借用页面执行一次性/批量操作的安全包装器
   *
   * 根据页面是否已在缓存中，采用不同的释放策略：
   *   - 已缓存：调用 release()，页面保持在缓存中并启动 TTL 定时器
   *   - 新加载：调用 disposeImmediate()，立即释放页面资源
 *
   * 这种设计避免了为临时操作（如获取页面文本、元数据等）
   * 污染缓存或触发不必要的 TTL 等待。
   *
   * @param pageIdx - 页面索引
   * @param fn - 要在页面上下文中执行的函数
   * @returns fn 的返回值
   */
  borrowPage<T>(pageIdx: number, fn: (ctx: PageContext) => T): T {
    const existed = this.cache.has(pageIdx);
    const ctx = this.acquire(pageIdx);
    try {
      return fn(ctx);
    } finally {
      // 根据页面是否之前就存在，选择不同的释放策略
      existed ? ctx.release() : ctx.disposeImmediate();
    }
  }

  /**
   * 强制立即释放所有缓存的页面
   *
   * 在文档关闭时调用，无视引用计数和 TTL，立即清理所有页面资源。
   */
  forceReleaseAll(): void {
    for (const ctx of this.cache.values()) {
      ctx.disposeImmediate();
    }
    this.cache.clear();
    // 清空 LRU 链表
    this.lruHead = null;
    this.lruTail = null;
    this.lruMap.clear();
  }

  /**
   * 更新缓存配置并应用到所有已缓存的页面
   *
   * @param config - 新的缓存配置
   */
  updateConfig(config: Required<CacheConfig>): void {
    this.config = config;

    // 更新所有已缓存页面的 TTL
    for (const ctx of this.cache.values()) {
      ctx.updateTtl(config.pageTtl);
    }

    // 如果新的容量上限更小，立即驱逐多余页面
    this.evictIfNeeded();
  }

  /**
   * 获取当前缓存中的页面数量
   */
  size(): number {
    return this.cache.size;
  }

  /**
   * 按需驱逐最久未使用的页面
   *
   * LRU 驱逐算法：
   *   1. 当缓存大小 >= maxPagesPerDocument 时触发
   *   2. 从 LRU 链表头部（最久未使用）开始检查
   *   3. 只驱逐引用计数为 0 的页面
   *   4. 如果 LRU 页面正在被使用，停止驱逐以避免无限循环
   *   5. 添加安全保护，防止死循环
   *
   * 注意：如果所有缓存的页面都在使用中，驱逐可能无法释放空间。
   * 这种情况下，新页面仍然会被加载（缓存暂时超过上限），
   * 待引用释放后会再次触发驱逐。
   */
  private evictIfNeeded(): void {
    // 添加安全保护，防止意外死循环
    let iterations = 0;
    const maxIterations = this.config.maxPagesPerDocument + 1;

    while (this.cache.size >= this.config.maxPagesPerDocument && iterations < maxIterations) {
      iterations++;

      // 从 LRU 链表头部获取最久未使用的页面
      const lruNode = this.lruHead;
      if (!lruNode) {
        // LRU 链表为空但缓存不为空（数据不一致），跳出循环
        break;
      }

      const lruPageIdx = lruNode.pageIdx;
      const ctx = this.cache.get(lruPageIdx);

      if (ctx) {
        // 只驱逐未被使用的页面（引用计数为 0）
        if (ctx.getRefCount() === 0) {
          ctx.disposeImmediate();
          // disposeImmediate 会触发 onFinalDispose 回调，
          // 从 cache 和 LRU 链表中移除该页面
        } else {
          // LRU 页面正在被使用，无法驱逐，跳出循环避免死循环
          break;
        }
      } else {
        // 页面不在缓存中但仍在 LRU 链表中（数据不一致），清理之
        this.removeFromAccessOrder(lruPageIdx);
      }
    }

    // 如果达到了最大迭代次数但仍然需要驱逐，说明可能存在问题
    // 强制清理 LRU 链表以恢复到一致状态
    if (iterations >= maxIterations && this.cache.size >= this.config.maxPagesPerDocument) {
      // 紧急情况：强制驱逐最久未使用的页面，即使引用计数不为零
      if (this.lruHead) {
        const lruPageIdx = this.lruHead.pageIdx;
        const ctx = this.cache.get(lruPageIdx);
        if (ctx) {
          ctx.disposeImmediate();
        }
      }
    }
  }

  /**
   * 更新页面的 LRU 访问顺序
   *
   * 将指定页面移到链表尾部（最近使用位置）。
   *
   * @param pageIdx - 要更新的页面索引
   */
  private updateAccessOrder(pageIdx: number): void {
    const node = this.lruMap.get(pageIdx);
    if (!node) {
      // 页面不在链表中，创建新节点并添加到尾部
      const newNode: LRUNode = {
        pageIdx,
        prev: this.lruTail,
        next: null,
      };
      this.lruMap.set(pageIdx, newNode);

      if (this.lruTail) {
        this.lruTail.next = newNode;
      } else {
        // 链表为空，新节点既是头也是尾
        this.lruHead = newNode;
      }
      this.lruTail = newNode;
    } else {
      // 页面已存在，移到尾部
      this.moveToTail(node);
    }
  }

  /**
   * 从 LRU 链表中移除指定页面（O(1) 操作）
   *
   * @param pageIdx - 要移除的页面索引
   */
  private removeFromAccessOrder(pageIdx: number): void {
    const node = this.lruMap.get(pageIdx);
    if (node) {
      this.removeNode(node);
      this.lruMap.delete(pageIdx);
    }
  }

  /**
   * 将指定节点移到链表尾部（最近使用位置）
   *
   * @param node - 要移动的节点
   */
  private moveToTail(node: LRUNode): void {
    if (node === this.lruTail) {
      // 已经在尾部，无需操作
      return;
    }

    // 从当前位置断开
    if (node.prev) {
      node.prev.next = node.next;
    } else {
      // node 是头节点
      this.lruHead = node.next;
    }

    if (node.next) {
      node.next.prev = node.prev;
    }

    // 添加到尾部
    node.prev = this.lruTail;
    node.next = null;

    if (this.lruTail) {
      this.lruTail.next = node;
    } else {
      // 链表为空
      this.lruHead = node;
    }
    this.lruTail = node;
  }

  /**
   * 从链表中移除指定节点（O(1) 操作）
   *
   * @param node - 要移除的节点
   */
  private removeNode(node: LRUNode): void {
    if (node.prev) {
      node.prev.next = node.next;
    } else {
      // node 是头节点
      this.lruHead = node.next;
    }

    if (node.next) {
      node.next.prev = node.prev;
    } else {
      // node 是尾节点
      this.lruTail = node.prev;
    }

    // 清理节点引用，帮助垃圾回收
    node.prev = null;
    node.next = null;
  }
}

/**
 * 页面上下文
 *
 * 管理单个 PDF 页面在 PDFium 中的完整生命周期，包括：
 *   - 引用计数（追踪页面是否正在被使用）
 *   - TTL 定时器（空闲超时后自动释放）
 *   - 懒加载的子资源（文本页等）
 *   - 安全的资源访问包装器（注释、表单句柄等）
 *
 * 生命周期状态转换：
 *   创建 → acquire（refCount > 0, 活跃） → release（refCount = 0, 等待 TTL）
 *        → TTL 超时 → disposeImmediate（已销毁）
 *   或：
 *   创建 → acquire → disposeImmediate（强制销毁）
 *
 * 子资源管理：
 *   页面可能关联多个子资源（文本页、注释、表单句柄等），
 *   这些资源在 disposeImmediate 时按正确顺序释放。
 */
export class PageContext {
  /** 引用计数，追踪当前有多少调用者正在使用此页面 */
  private refCount = 0;
  /** TTL 超时定时器，refCount 降为 0 时启动 */
  private expiryTimer?: ReturnType<typeof setTimeout>;
  /** 是否已销毁的标志，防止重复释放 */
  private disposed = false;
  /** 当前生效的生存时间（毫秒） */
  private ttl: number;

  /**
   * 懒加载的文本页指针
   *
   * 文本页（FPDF_TEXTPAGE）用于文本提取操作（搜索、选择、复制等）。
   * 只有在首次调用 getTextPage() 时才会加载，避免不需要文本操作时的开销。
   */
  private textPagePtr?: number;

  /**
   * @param pdf - PDFium WASM 模块封装
   * @param docPtr - 所属文档的 PDFium 句柄
   * @param pageIdx - 页面索引（从 0 开始）
   * @param pagePtr - PDFium 页面句柄指针（FPDF_PAGE）
   * @param ttl - 页面空闲后的生存时间（毫秒）
   * @param onFinalDispose - 页面被最终销毁时的回调函数
   *                          负责从外部缓存结构中移除此页面的引用
   */
  constructor(
    private readonly pdf: WrappedPdfiumModule,
    public readonly docPtr: number,
    public readonly pageIdx: number,
    public readonly pagePtr: number,
    ttl: number,
    private readonly onFinalDispose: () => void,
  ) {
    this.ttl = ttl;
  }

  /**
   * 增加引用计数
   *
   * 由 PageCache.acquire() 调用。每次 acquire 都会增加引用计数，
   * 确保页面在被使用期间不会被驱逐或过期。
   *
   * @throws 如果页面已被销毁，抛出错误
   */
  bumpRefCount() {
    if (this.disposed) throw new Error('Context already disposed');
    this.refCount++;
  }

  /**
   * 获取当前引用计数
   *
   * 主要供 PageCache 的 LRU 驱逐逻辑使用，
   * 判断页面是否可以安全驱逐（refCount === 0）。
   */
  getRefCount(): number {
    return this.refCount;
  }

  /**
   * 清除 TTL 过期定时器
   *
   * 由 PageCache.acquire() 调用。当页面被重新获取时，
   * 取消之前可能正在倒计时的过期定时器，防止正在使用的页面被意外回收。
   */
  clearExpiryTimer() {
    if (this.expiryTimer) {
      clearTimeout(this.expiryTimer);
      this.expiryTimer = undefined;
    }
  }

  /**
   * 更新 TTL 配置
   *
   * 如果页面当前有空闲定时器在运行（refCount === 0 且有 expiryTimer），
   * 则用新的 TTL 重新启动定时器。
   *
   * @param newTtl - 新的生存时间（毫秒）
   */
  updateTtl(newTtl: number): void {
    this.ttl = newTtl;
    // 如果有空闲定时器在运行，用新 TTL 重启
    if (this.expiryTimer && this.refCount === 0) {
      this.clearExpiryTimer();
      this.expiryTimer = setTimeout(() => this.disposeImmediate(), this.ttl);
    }
  }

  /**
   * 释放页面引用（减少引用计数）
   *
   * 当调用者完成对页面的操作后调用。当引用计数降为 0 时，
   * 启动 TTL 定时器，超时后自动清理页面资源。
   *
   * 如果页面已被销毁（可能因 forceReleaseAll），静默返回。
   */
  release() {
    if (this.disposed) return;
    this.refCount--;
    if (this.refCount === 0) {
      // 引用计数归零，启动 TTL 定时器
      // 定时器触发后将调用 disposeImmediate() 彻底释放资源
      this.expiryTimer = setTimeout(() => this.disposeImmediate(), this.ttl);
    }
  }

  /**
   * 立即销毁页面及其所有子资源
   *
   * 无视引用计数和 TTL，立即释放所有资源。
   * 释放顺序（顺序很重要，避免悬空指针）：
   *   1. 清除可能存在的 TTL 定时器
   *   2. 关闭文本页（如果已加载）
   *   3. 关闭 PDFium 页面句柄
   *   4. 调用 onFinalDispose 回调，从外部缓存结构中移除引用
   */
  disposeImmediate() {
    if (this.disposed) return;
    this.disposed = true;

    // 1. 清除可能存在的 TTL 定时器
    this.clearExpiryTimer();

    // 2. 关闭文本页（如果已懒加载）
    if (this.textPagePtr !== undefined) {
      this.pdf.FPDFText_ClosePage(this.textPagePtr);
    }

    // 3. 关闭 PDFium 页面句柄
    this.pdf.FPDF_ClosePage(this.pagePtr);

    // 4. 通知外部缓存结构移除此页面的引用
    this.onFinalDispose();
  }

  // ── 公共辅助方法 ──

  /**
   * 获取（或懒加载）文本页句柄
   *
   * 文本页（FPDF_TEXTPAGE）是 PDFium 中用于文本相关操作的对象，
   * 包括文本提取、搜索、选择等。创建文本页有一定开销，因此采用懒加载策略。
   *
   * 此方法是幂等的：多次调用只会在首次时创建文本页，后续调用直接返回缓存值。
   *
   * @returns PDFium 文本页句柄指针
   * @throws 如果页面已被销毁
   */
  getTextPage(): number {
    this.ensureAlive();
    if (this.textPagePtr === undefined) {
      // 首次调用时加载文本页
      this.textPagePtr = this.pdf.FPDFText_LoadPage(this.pagePtr);
    }
    return this.textPagePtr;
  }

  /**
   * 安全地执行注释操作
   *
   * 提供一个 RAII 风格的注释访问包装器，确保注释句柄在使用后被正确关闭。
   * 无论 fn 是否抛出异常，注释句柄都会在 finally 块中被关闭。
   *
   * @param annotIdx - 注释索引（页面内的注释序号）
   * @param fn - 接受注释句柄指针的回调函数
   * @returns fn 的返回值
   * @throws 如果页面已被销毁
   */
  withAnnotation<T>(annotIdx: number, fn: (annotPtr: number) => T): T {
    this.ensureAlive();
    const annotPtr = this.pdf.FPDFPage_GetAnnot(this.pagePtr, annotIdx);
    try {
      return fn(annotPtr);
    } finally {
      // 确保注释句柄被关闭，防止资源泄漏
      this.pdf.FPDFPage_CloseAnnot(annotPtr);
    }
  }

  /**
   * 安全地执行表单操作
   *
   * 提供一个 RAII 风格的表单环境包装器。每次调用都创建全新的表单环境，
   * 使用后立即销毁，不缓存，避免状态过期问题。
   *
   * 表单环境的创建和销毁步骤：
   *   1. 创建表单填充信息结构（FormFillInfo）
   *   2. 初始化表单填充环境（FormHandle）
   *   3. 通知 PDFium 页面已加载到表单环境中
   *   4. 执行用户操作
   *   5. 通知 PDFium 页面即将从表单环境中卸载
   *   6. 销毁表单填充环境
   *   7. 关闭表单填充信息结构
   *
   * 设计决策：不缓存表单句柄的原因是表单状态（如焦点、编辑状态等）
   * 可能在操作之间发生变化，使用缓存的句柄可能导致不一致的行为。
   *
   * @param fn - 接受表单句柄指针的回调函数
   * @returns fn 的返回值
   * @throws 如果页面已被销毁
   */
  withFormHandle<T>(fn: (formHandle: number) => T): T {
    this.ensureAlive();
    // 创建表单填充信息结构
    const formInfoPtr = this.pdf.PDFiumExt_OpenFormFillInfo();
    // 初始化表单填充环境（绑定到文档）
    const formHandle = this.pdf.PDFiumExt_InitFormFillEnvironment(this.docPtr, formInfoPtr);
    // 通知页面已被加载到表单环境
    this.pdf.FORM_OnAfterLoadPage(this.pagePtr, formHandle);
    try {
      return fn(formHandle);
    } finally {
      // 按相反顺序清理：先卸载页面，再销毁环境，最后关闭信息结构
      this.pdf.FORM_OnBeforeClosePage(this.pagePtr, formHandle);
      this.pdf.PDFiumExt_ExitFormFillEnvironment(formHandle);
      this.pdf.PDFiumExt_CloseFormFillInfo(formInfoPtr);
    }
  }

  /**
   * 确保页面上下文仍然有效（未被销毁）
   *
   * 所有公共辅助方法在执行前都会调用此方法，防止在已销毁的页面上执行操作。
   *
   * @throws 如果页面已被销毁
   */
  private ensureAlive() {
    if (this.disposed) throw new Error('PageContext already disposed');
  }
}
