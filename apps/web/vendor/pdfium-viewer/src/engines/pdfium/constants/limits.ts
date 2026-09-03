/**
 * @file 系统限制与安全阈值常量
 *
 * 整体作用：
 *   集中定义 PDFium 引擎运行时的各类资源限制常量。
 *   这些限制用于在 MemoryManager 等模块中进行边界校验，
 *   防止因恶意/畸形 PDF 文件导致以下问题：
 *     - 内存耗尽（Memory exhaustion）
 *     - 浏览器标签页崩溃（Browser crashes）
 *     - 拒绝服务攻击（DoS attacks）
 *     - WASM 线性堆溢出（WASM heap overflow）
 *     - 不合理的资源占用（Unreasonable resource usage）
 *
 * 设计决策：
 *   - 所有常量集中在一个文件中，便于统一调整和维护。
 *   - 使用 `as const` 断言确保常量不可变，并在类型层面收窄为字面量类型。
 *   - 导出 Limits 类型，允许其他模块在类型注解中引用这些常量的结构。
 *
 * @module constants/limits
 */

/**
 * 内存分配限制
 *
 * MAX_TOTAL_MEMORY (2GB)：
 *   PDFium WASM 模块运行在浏览器中，可用的线性内存空间有限。
 *   2GB 是一个安全上限，既留有足够的渲染空间，又能防止单个 PDF
 *   占用过多内存导致浏览器标签页崩溃。这个值对应 WASM 线性内存
 *   的理论最大值（约 4GB）的一半，留有余量给 WASM 运行时自身开销。
 */
export const MEMORY_LIMITS = {
  /** 允许分配的最大总内存量：2GB（2 * 1024 * 1024 * 1024 字节） */
  MAX_TOTAL_MEMORY: 2 * 1024 * 1024 * 1024,
} as const;

/**
 * 所有限制常量的聚合对象
 *
 * 采用嵌套结构（MEMORY / ...）便于后续扩展其他维度的限制，
 * 例如：页面尺寸限制、渲染像素上限、并发任务数等。
 * 使用方式：LIMITS.MEMORY.MAX_TOTAL_MEMORY
 */
export const LIMITS = {
  MEMORY: MEMORY_LIMITS,
} as const;

/**
 * 限制常量的 TypeScript 类型
 *
 * 通过 typeof 反向推导，确保类型定义与实际常量值始终保持同步。
 * 其他模块可使用此类型来声明依赖限制常量的参数或属性。
 */
export type Limits = typeof LIMITS;
