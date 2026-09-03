/**
 * @file 图像转换器类型定义
 *
 * 定义了 ImageDataConverter 接口 — 将原始像素数据（RGBA ImageData）
 * 转换为目标格式（PNG Blob、BMP Blob 等）的可调用对象。
 *
 * 设计为延迟求值（LazyImageData）模式：
 *   调用 getImageData() 时才获取像素数据，避免在排队等待期间占用内存。
 */

import type { PdfImage, ImageConversionTypes } from '../../models';

// 从 models 便捷重导出
export type { ImageConversionTypes } from '../../models';

/**
 * 延迟图像数据获取函数
 *
 * 返回包含 RGBA 像素数据和尺寸信息的 PdfImage 对象。
 * 使用函数而非直接值，是为了在任务实际执行时才获取数据（延迟求值），
 * 避免在队列中排队时就占用大量像素内存。
 */
export type LazyImageData = () => PdfImage;

/**
 * 图像数据转换器接口
 *
 * 将原始 RGBA 像素数据编码为目标格式（如 PNG Blob）。
 * 是一个可调用对象（函数式接口），同时支持可选的 destroy 方法。
 *
 * 实现策略：
 *   - 浏览器环境：使用 OffscreenCanvas（Worker）或 Canvas（主线程）
 *   - Node.js 环境：可使用 Sharp 等图像处理库
 *
 * @template T - 输出格式（通常为 Blob）
 */
export interface ImageDataConverter<T = Blob> {
  /**
   * 将原始像素数据编码为目标格式
   *
   * @param getImageData  - 延迟获取像素数据的函数
   * @param imageType     - 目标图像格式（默认 'image/png'）
   * @param imageQuality  - 编码质量（0-1，仅对有损格式有效）
   * @returns 编码后的数据（如 PNG Blob）
   */
  (
    getImageData: LazyImageData,
    imageType?: ImageConversionTypes,
    imageQuality?: number,
  ): Promise<T>;

  /**
   * 可选的资源清理方法
   *
   * 当 PdfEngine.destroy() 被调用时触发，用于释放 Worker 池等资源。
   */
  destroy?: () => void;
}
