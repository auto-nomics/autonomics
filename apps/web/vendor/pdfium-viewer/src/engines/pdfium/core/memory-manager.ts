/**
 * @file 内存管理器 — 对 WASM 堆内存分配进行追踪、验证与泄漏检测
 *
 * 整体作用：
 *   PDFium 以 WASM 形式运行，其内部分配的内存位于线性堆（linear memory）中，
 *   JavaScript 端无法依赖 GC 自动回收。本模块封装了 malloc / free 调用，
 *   在每次分配时记录指针、大小、时间戳和调用栈，从而实现：
 *     1. 全局内存用量上限保护（防止 WASM 堆溢出导致浏览器崩溃）
 *     2. 分配/释放配对追踪（检测未释放的内存泄漏）
 *     3. 调试友好的统计接口（仅在 debug 模式下输出详细调用栈）
 *
 * 设计决策：
 *   - 使用 branded type（WasmPointer）替代裸 number，以在编译期区分
 *     "普通数字"和"WASM 指针"，避免误传。
 *   - 分配记录采用 Map<number, Allocation>，以指针地址为 key，实现 O(1) 查找。
 *   - 仅在 logger 开启 debug 级别时才捕获 Error().stack，因为堆栈抓取
 *     在高频路径上开销较大。
 */

import { WasmPointer } from '../types/branded';
import { LIMITS } from '../constants/limits';
import type { WrappedPdfiumModule } from '../../../pdfium';
import { Logger } from '../../../models';

/** 日志来源标识，用于在日志中标记消息来自 PDFium 引擎 */
const LOG_SOURCE = 'PDFiumEngine';
/** 日志分类标识，用于在日志中标记消息来自内存管理器 */
const LOG_CATEGORY = 'MemoryManager';

/**
 * 单次内存分配的追踪记录
 *
 * @property ptr       - WASM 堆上的内存起始地址（branded number）
 * @property size      - 本次分配的字节数
 * @property timestamp - 分配时的 Unix 时间戳（毫秒）
 * @property stack     - 分配时的 JS 调用栈（仅 debug 模式下捕获，用于泄漏定位）
 */
interface Allocation {
  ptr: WasmPointer;
  size: number;
  timestamp: number;
  stack?: string;
}

/**
 * 内存管理器 — 封装 WASM 的 malloc/free，提供分配追踪与泄漏检测
 *
 * 典型用法：
 * ```ts
 * const mem = new MemoryManager(pdfiumModule, logger);
 * const ptr = mem.malloc(1024);      // 分配 1KB，自动记录
 * // ... 使用 ptr 读写 WASM 内存 ...
 * mem.free(ptr);                      // 释放并移除记录
 * mem.checkLeaks();                   // 在销毁前检查是否有未释放的内存
 * ```
 */
export class MemoryManager {
  /**
   * 所有活跃（已分配但尚未释放）的内存记录
   * key = 指针地址（number），value = 分配详情
   */
  private allocations = new Map<number, Allocation>();

  /**
   * 当前已分配但未释放的总字节数
   * 用于与 LIMITS.MEMORY.MAX_TOTAL_MEMORY 做上限比较
   */
  private totalAllocated = 0;

  /**
   * @param pdfiumModule - 已初始化的 PDFium WASM 模块实例，提供 wasmExports.malloc / free
   * @param logger       - 日志记录器，用于输出警告和调试信息
   */
  constructor(
    private pdfiumModule: WrappedPdfiumModule,
    private logger: Logger,
  ) {}

  /**
   * 分配指定大小的 WASM 堆内存，并进行上限校验和分配追踪
   *
   * 工作流程：
   *   1. 检查分配后总内存是否超过全局上限（2GB），超限则抛出异常
   *   2. 调用 WASM 的 malloc 获取堆指针
   *   3. 将分配信息记录到 allocations Map 中
   *   4. 累加 totalAllocated 计数器
   *
   * @param size - 需要分配的字节数，必须为正整数
   * @returns branded 类型的 WASM 指针，不可与普通 number 混用
   * @throws 如果总内存超限或 malloc 返回空指针（内存不足）
   */
  malloc(size: number): WasmPointer {
    // 检查总内存用量：防止累计分配超过 2GB 上限，避免 WASM 堆溢出
    if (this.totalAllocated + size > LIMITS.MEMORY.MAX_TOTAL_MEMORY) {
      throw new Error(
        `Total memory usage would exceed limit: ` +
          `${this.totalAllocated + size} > ${LIMITS.MEMORY.MAX_TOTAL_MEMORY}`,
      );
    }

    // 调用 WASM 导出的 malloc 函数进行实际内存分配
    const ptr = this.pdfiumModule.pdfium.wasmExports.malloc(size);

    // malloc 返回 0 表示分配失败（WASM 堆空间不足）
    if (!ptr) {
      throw new Error(`Failed to allocate ${size} bytes`);
    }

    // 构造分配记录，仅在 debug 模式下捕获调用栈（性能考量）
    const allocation: Allocation = {
      ptr: WasmPointer(ptr),
      size,
      timestamp: Date.now(),
      stack: this.logger.isEnabled('debug') ? new Error().stack : undefined,
    };

    // 将分配记录存入 Map，供后续 free 时查找和 checkLeaks 时使用
    this.allocations.set(ptr, allocation);
    // 累加已分配总量
    this.totalAllocated += size;

    return WasmPointer(ptr);
  }

  /**
   * 释放先前通过 malloc 分配的 WASM 堆内存
   *
   * 工作流程：
   *   1. 在 allocations Map 中查找该指针的分配记录
   *   2. 如果未找到记录，输出警告（可能是双重释放或释放了外部管理的指针）
   *   3. 如果找到记录，从总量中扣减并移除记录
   *   4. 无论是否找到记录，都调用 WASM 的 free 释放底层内存
   *
   * 注意：即使指针未被追踪，仍然会调用底层 free，以确保 WASM 侧内存被释放。
   * 这种"宽容释放"策略可以避免因追踪遗漏导致的真实内存泄漏。
   *
   * @param ptr - 先前 malloc 返回的 branded WASM 指针
   */
  free(ptr: WasmPointer): void {
    const allocation = this.allocations.get(ptr);
    if (!allocation) {
      // 指针不在追踪记录中：可能是双重释放，或该指针由外部代码分配
      this.logger.warn(LOG_SOURCE, LOG_CATEGORY, `Freeing untracked pointer: ${ptr}`);
    } else {
      // 从已分配总量中扣减该次分配的大小
      this.totalAllocated -= allocation.size;
      // 从追踪 Map 中移除该记录
      this.allocations.delete(ptr);
    }

    // 调用 WASM 导出的 free 函数释放底层内存
    this.pdfiumModule.pdfium.wasmExports.free(ptr);
  }

  /**
   * 获取当前内存分配的统计数据
   *
   * 返回值中 allocations 数组仅在 debug 模式下填充，
   * 避免在生产环境中暴露大量分配详情影响性能。
   *
   * @returns 包含总分配字节数、活跃分配数量和（可选）详细分配列表的对象
   */
  getStats() {
    return {
      /** 当前已分配但未释放的总字节数 */
      totalAllocated: this.totalAllocated,
      /** 当前活跃的分配数量（未释放的 malloc 调用次数） */
      allocationCount: this.allocations.size,
      /** 详细的分配记录列表（仅 debug 模式下返回，生产环境为空数组） */
      allocations: this.logger.isEnabled('debug') ? Array.from(this.allocations.values()) : [],
    };
  }

  /**
   * 检测内存泄漏 — 检查是否存在已分配但未释放的内存
   *
   * 通常在 PDF 文档关闭或引擎销毁时调用。如果存在未释放的分配，
   * 会输出每个泄漏指针的地址、大小和（如果可用）分配时的调用栈，
   * 方便开发者定位泄漏来源。
   *
   * 注意：此方法仅输出警告，不会抛出异常，也不会自动释放内存。
   */
  checkLeaks(): void {
    if (this.allocations.size > 0) {
      this.logger.warn(
        LOG_SOURCE,
        LOG_CATEGORY,
        `Potential memory leak: ${this.allocations.size} unfreed allocations`,
      );

      // 遍历所有未释放的分配，逐一输出详情
      for (const [ptr, alloc] of this.allocations) {
        // alloc.stack 包含分配时的 JS 调用栈（仅 debug 模式），可直接定位泄漏代码位置
        this.logger.warn(LOG_SOURCE, LOG_CATEGORY, `  - ${ptr}: ${alloc.size} bytes`, alloc.stack);
      }
    }
  }
}
