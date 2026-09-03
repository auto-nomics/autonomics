/**
 * @file 引擎层通用工具函数
 *
 * 提供 ArrayBuffer 视图和像素数据格式转换的工具函数，
 * 用于在 WASM 内存和 JavaScript 之间安全地传递二进制数据。
 */

/**
 * 将任意 ArrayBufferView 转换为独立的 ArrayBuffer
 *
 * 处理两种情况：
 *   1. 视图底层是普通 ArrayBuffer → 使用 slice() 切片（零拷贝语义）
 *   2. 视图底层是 SharedArrayBuffer 或其他类型 → 复制一份到新 ArrayBuffer
 *
 * 返回的 ArrayBuffer 保证是独立的（不共享内存），可以安全地 transfer 到 Worker。
 *
 * @param view - 任意类型化数组视图（Uint8Array、Int32Array 等）
 * @returns 独立的 ArrayBuffer，包含视图范围内的数据
 */
export function toArrayBuffer(view: ArrayBufferView): ArrayBuffer {
  const { buffer, byteOffset, byteLength } = view;
  if (buffer instanceof ArrayBuffer) {
    // 底层是普通 ArrayBuffer，直接切片即可
    return buffer.slice(byteOffset, byteOffset + byteLength);
  }
  // SharedArrayBuffer 或未知类型 → 复制到新的普通 ArrayBuffer
  const ab = new ArrayBuffer(byteLength);
  new Uint8Array(ab).set(new Uint8Array(buffer as ArrayBufferLike, byteOffset, byteLength));
  return ab;
}

/**
 * 确保返回一个以普通 ArrayBuffer 为底层的 Uint8ClampedArray
 *
 * PDFium 渲染输出的像素数据需要是 RGBA 格式的 Uint8ClampedArray。
 * 此函数处理输入数据可能是各种 TypedArray + SharedArrayBuffer 的情况，
 * 统一转换为 ImageData 构造函数所需的格式。
 *
 * @param data - 任意像素数据（可能是 Uint8Array、Uint8ClampedArray 等）
 * @returns 以普通 ArrayBuffer 为底层的 Uint8ClampedArray
 */
export function toClampedRGBA(
  data: ArrayBufferView | Uint8Array | Uint8ClampedArray,
): Uint8ClampedArray {
  // 如果已经是正确格式，直接返回（避免不必要的复制）
  if (data instanceof Uint8ClampedArray && data.buffer instanceof ArrayBuffer) return data;
  // 否则复制到新的 Uint8ClampedArray
  return new Uint8ClampedArray(toArrayBuffer(data as ArrayBufferView));
}
