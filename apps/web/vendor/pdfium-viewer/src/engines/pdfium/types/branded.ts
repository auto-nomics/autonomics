/**
 * @file Branded 类型 — 为 WASM 指针提供编译期类型安全
 *
 * 整体作用：
 *   在 TypeScript 中，WASM 指针本质上是一个 number，但将其与普通数字
 *   混用是非常危险的（例如将指针误传给期望数组索引的函数）。
 *   本文件通过 "branded type"（品牌类型）技术，在编译期将 WASM 指针
 *   与普通 number 区分开来，防止类型误用。
 *
 * 技术原理：
 *   TypeScript 的 "branding" 模式利用交叉类型（intersection type）
 *   将一个唯一符号标记附加到基础类型上：
 *     type WasmPointer = number & { [PointerBrand]: never }
 *   这样 WasmPointer 在运行时仍然是 number（附加的符号属性不会实际存在），
 *   但在编译时 TypeScript 会将其视为与 number 不同的类型，
 *   必须通过显式的 WasmPointer() 转换函数才能获得。
 *
 * @public 导出供其他模块使用
 */

/**
 * 品牌标记符号 — 用于在类型层面区分 WasmPointer 与普通 number
 *
 * 声明为 `unique symbol` 确保全局唯一，无法从外部伪造。
 * `declare const` 表示此符号仅在类型系统中使用，运行时不存在。
 */
declare const PointerBrand: unique symbol;

/**
 * WASM 指针的品牌类型
 *
 * 这是本文件的核心类型。它在 number 基础上附加了一个唯一符号属性，
 * 使得 TypeScript 编译器能区分 "WASM 堆指针" 和 "普通数字"。
 *
 * 使用场景：
 *   - MemoryManager.malloc() 返回此类型（而非裸 number）
 *   - MemoryManager.free() 接受此类型作为参数
 *   - 所有需要操作 WASM 内存地址的函数都应使用此类型
 *
 * 注意：此类型在运行时完全等价于 number，不产生任何额外开销。
 */
export type WasmPointer = number & { [PointerBrand]: never };

/**
 * WASM 指针的构造函数 — 将普通 number 转换为 branded WasmPointer
 *
 * 此函数是获得 WasmPointer 类型值的唯一合法途径（除类型断言外）。
 * 它在运行时是一个恒等函数（直接返回输入值），仅用于类型转换。
 *
 * 设计意图：
 *   - 集中指针转换逻辑，便于代码审查时快速定位所有指针来源
 *   - 与 class 同名的导出函数（TypeScript 模式），提供工厂语义
 *
 * @param ptr - WASM 堆上的原始地址（number）
 * @returns 同一个数值，但类型标记为 WasmPointer
 */
export const WasmPointer = (ptr: number): WasmPointer => ptr as WasmPointer;
