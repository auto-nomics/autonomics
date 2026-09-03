/**
 * @file 浏览器环境图像转换器
 *
 * 提供三种转换策略，将 RGBA 像素数据编码为 PNG/BMP 等格式的 Blob：
 *   1. browserImageDataToBlobConverter — 主线程 Canvas 编码（简单但阻塞）
 *   2. createWorkerPoolImageConverter — Worker 线程 OffscreenCanvas 编码（非阻塞）
 *   3. createHybridImageConverter — 混合模式：Worker 优先，失败时降级到主线程
 */

import type { ImageConversionTypes } from '../../models';
import type { ImageDataConverter, LazyImageData } from './types';
import { ImageEncoderWorkerPool } from '../image-encoder';
import { rgbaToBmpBlob } from '../image-encoder/bmp';

// ============================================================================
// 错误类
// ============================================================================

/** 图像转换过程中的错误 */
export class ImageConverterError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ImageConverterError';
  }
}

// ============================================================================
// 浏览器转换器实现
// ============================================================================

/**
 * 主线程 Canvas 转换器 — 使用 HTML Canvas API 编码图像
 *
 * 特点：
 *   - 兼容性好，所有浏览器都支持
 *   - 编码过程在主线程执行，会阻塞 UI
 *   - 适合作为 Worker 编码不可用时的降级方案
 *
 * BMP 快速路径：
 *   BMP 格式无需 Canvas 操作，直接在内存中拼接文件头 + 原始像素数据，
 *   避免了 Canvas 的开销，是最快的编码路径。
 */
export const browserImageDataToBlobConverter: ImageDataConverter<Blob> = (
  getImageData: LazyImageData,
  imageType: ImageConversionTypes = 'image/png',
  quality?: number,
): Promise<Blob> => {
  const pdfImage = getImageData();

  // 快速路径：BMP 无需 Canvas，直接拼接文件头 + 原始 RGBA 像素
  if (imageType === 'image/bmp') {
    return Promise.resolve(rgbaToBmpBlob(pdfImage.data, pdfImage.width, pdfImage.height));
  }

  // 非浏览器环境检查
  if (typeof document === 'undefined') {
    return Promise.reject(
      new ImageConverterError(
        'document is not available. This converter requires a browser environment.',
      ),
    );
  }

  // 使用 Canvas API 编码：putImageData → toBlob
  const imageData = new ImageData(pdfImage.data, pdfImage.width, pdfImage.height);

  return new Promise((resolve, reject) => {
    const canvas = document.createElement('canvas');
    canvas.width = imageData.width;
    canvas.height = imageData.height;
    canvas.getContext('2d')!.putImageData(imageData, 0, 0);

    canvas.toBlob(
      (blob) => {
        if (blob) {
          resolve(blob);
        } else {
          reject(new ImageConverterError('Canvas toBlob returned null'));
        }
      },
      imageType,
      quality,
    );
  });
};

/**
 * Worker 池转换器 — 在独立 Worker 线程中使用 OffscreenCanvas 编码
 *
 * 特点：
 *   - 非阻塞，编码在 Worker 线程中执行
 *   - 支持并行编码（多个 Worker 同时处理）
 *   - 需要浏览器支持 OffscreenCanvas
 *
 * @param workerPool - 已创建的图像编码 Worker 池实例
 * @returns 带有 destroy() 方法的转换器函数
 */
export function createWorkerPoolImageConverter(
  workerPool: ImageEncoderWorkerPool,
): ImageDataConverter<Blob> {
  const converter: ImageDataConverter<Blob> = (
    getImageData: LazyImageData,
    imageType: ImageConversionTypes = 'image/png',
    quality?: number,
  ): Promise<Blob> => {
    const pdfImage = getImageData();

    // BMP 快速路径：无需 Worker 通信
    if (imageType === 'image/bmp') {
      return Promise.resolve(rgbaToBmpBlob(pdfImage.data, pdfImage.width, pdfImage.height));
    }

    // 复制像素数据（原始数据可能在 WASM 堆上，不能直接 transfer）
    const dataCopy = new Uint8ClampedArray(pdfImage.data);

    // 提交给 Worker 池编码
    return workerPool.encode(
      {
        data: dataCopy,
        width: pdfImage.width,
        height: pdfImage.height,
      },
      imageType,
      quality,
    );
  };

  // 附加清理方法，用于销毁 Worker 池
  converter.destroy = () => workerPool.destroy();

  return converter;
}

/**
 * 混合转换器 — Worker 优先 + 主线程降级
 *
 * 推荐的生产环境方案：
 *   1. 首先尝试 Worker 池编码（OffscreenCanvas，非阻塞）
 *   2. 如果 Worker 编码失败（如浏览器不支持 OffscreenCanvas），降级到主线程 Canvas
 *
 * @param workerPool - 已创建的图像编码 Worker 池实例
 * @returns 带有 destroy() 方法的转换器函数
 */
export function createHybridImageConverter(
  workerPool: ImageEncoderWorkerPool,
): ImageDataConverter<Blob> {
  const converter: ImageDataConverter<Blob> = async (
    getImageData: LazyImageData,
    imageType: ImageConversionTypes = 'image/png',
    quality?: number,
  ): Promise<Blob> => {
    const pdfImage = getImageData();

    // BMP 快速路径
    if (imageType === 'image/bmp') {
      return rgbaToBmpBlob(pdfImage.data, pdfImage.width, pdfImage.height);
    }

    try {
      // 首选：Worker 池编码（OffscreenCanvas，非阻塞）
      const dataCopy = new Uint8ClampedArray(pdfImage.data);

      return await workerPool.encode(
        {
          data: dataCopy,
          width: pdfImage.width,
          height: pdfImage.height,
        },
        imageType,
        quality,
      );
    } catch (error) {
      // 降级：主线程 Canvas 编码
      console.warn('Worker encoding failed, falling back to main-thread Canvas:', error);
      return browserImageDataToBlobConverter(getImageData, imageType, quality);
    }
  };

  converter.destroy = () => workerPool.destroy();

  return converter;
}
