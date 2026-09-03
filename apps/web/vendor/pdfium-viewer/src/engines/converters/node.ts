/**
 * @file Node.js 图像格式转换器
 *
 * 提供 Node.js 环境下的图像编码实现，支持三种方案：
 *   1. Sharp 转换器：基于 Sharp 库的高性能编码（推荐）
 *   2. Canvas 转换器：基于 node-canvas 库的编码
 *   3. 自定义转换器：接受任意图像处理库的通用接口
 *
 * 注意：Sharp 和 canvas 均为可选依赖，不随包自动安装。
 * 使用者需要自行安装所需的库并传入对应的工厂函数。
 */

import type { PdfImage, ImageConversionTypes } from '../../models';
import { toArrayBuffer } from '../utils';
import type { ImageDataConverter, LazyImageData } from './types';

// ============================================================================
// Node.js 图像转换器
// ============================================================================

/**
 * 基于 Sharp 的 Node.js 图像编码器
 *
 * 使用 Sharp 库将 RGBA 像素数据转换为 WebP/PNG/JPEG 格式的 Buffer。
 * Sharp 基于 libvips，性能优异，是 Node.js 环境下推荐的编码方案。
 *
 * @param sharp - Sharp 模块的引用（避免硬依赖，由调用方传入）
 * @returns ImageDataConverter<Buffer> 转换器实例
 *
 * @example
 * ```typescript
 * import sharp from 'sharp';
 * const converter = createNodeImageDataToBufferConverter(sharp);
 * ```
 */
export function createNodeImageDataToBufferConverter(
  sharp: any, // Using 'any' to avoid requiring sharp as a dependency
): ImageDataConverter<Buffer> {
  return async (
    getImageData: LazyImageData,
    imageType: ImageConversionTypes = 'image/webp',
    imageQuality?: number,
  ): Promise<Buffer> => {
    const imageData = getImageData();
    const { width, height, data } = imageData;

    // 将 ImageData 的 Uint8ClampedArray 转为 Node.js Buffer
    // Sharp 接受 RGBA 原始像素格式（4 通道，每通道 8 位）
    let sharpInstance = sharp(Buffer.from(data), {
      raw: {
        width,
        height,
        channels: 4, // RGBA 四通道
      },
    });

    // 根据目标格式选择对应的 Sharp 编码器
    let buffer: Buffer;
    switch (imageType) {
      case 'image/webp':
        buffer = await sharpInstance
          .webp({
            quality: imageQuality,
          })
          .toBuffer();
        break;
      case 'image/png':
        buffer = await sharpInstance.png().toBuffer();
        break;
      case 'image/jpeg':
        // JPEG 不支持透明通道，需要将 alpha 合成到白色背景上
        buffer = await sharpInstance
          .flatten({ background: { r: 255, g: 255, b: 255 } }) // 用白色替换透明区域
          .jpeg({
            quality: imageQuality,
          })
          .toBuffer();
        break;
      default:
        throw new Error(`Unsupported image type: ${imageType}`);
    }

    return buffer;
  };
}

/**
 * 基于 node-canvas 的 Node.js 图像编码器
 *
 * 使用 node-canvas 库将 RGBA 像素数据绘制到 Canvas 上，
 * 再调用 Canvas 的 toBuffer 方法输出为指定格式的 Buffer。
 *
 * @param createCanvas - node-canvas 的 createCanvas 工厂函数
 * @returns ImageDataConverter<Buffer> 转换器实例
 *
 * @example
 * ```typescript
 * import { createCanvas } from 'canvas';
 * const converter = createNodeCanvasImageDataToBlobConverter(createCanvas);
 * ```
 */
export function createNodeCanvasImageDataToBlobConverter(
  createCanvas: any, // Using 'any' to avoid requiring canvas as a dependency
): ImageDataConverter<Buffer> {
  return async (
    getImageData: LazyImageData,
    imageType: ImageConversionTypes = 'image/webp',
    _imageQuality?: number,
  ): Promise<Buffer> => {
    const imageData = getImageData();
    const { width, height } = imageData;

    // 创建 Canvas 并将像素数据写入
    const canvas = createCanvas(width, height);
    const ctx = canvas.getContext('2d');
    ctx.putImageData(imageData, 0, 0);

    // 根据目标格式输出 Buffer
    let buffer: Buffer;
    switch (imageType) {
      case 'image/webp':
        buffer = canvas.toBuffer('image/webp');
        break;
      case 'image/png':
        buffer = canvas.toBuffer('image/png');
        break;
      case 'image/jpeg':
        buffer = canvas.toBuffer('image/jpeg');
        break;
      default:
        throw new Error(`Unsupported image type: ${imageType}`);
    }

    return buffer;
  };
}

/**
 * 自定义图像编码器（返回 Blob）
 *
 * 提供一个通用的转换器工厂，接受任意的图像处理函数。
 * 处理函数接收原始 PdfImage 数据，返回编码后的 Buffer，
 * 工厂会自动将 Buffer 包装为 Blob。
 *
 * @param processor - 自定义图像处理函数，接收像素数据和格式参数，返回编码后的 Buffer
 * @returns ImageDataConverter 转换器实例（输出类型为 Blob）
 *
 * @example
 * ```typescript
 * const converter = createCustomImageDataToBlobConverter(async (imageData) => {
 *   return myImageLibrary.encode(imageData);
 * });
 * ```
 */
export function createCustomImageDataToBlobConverter(
  processor: (
    imageData: PdfImage,
    imageType?: ImageConversionTypes,
    imageQuality?: number,
  ) => Promise<Buffer>,
): ImageDataConverter {
  return async (
    getImageData: LazyImageData,
    imageType: ImageConversionTypes = 'image/webp',
    imageQuality?: number,
  ) => {
    const imageData = getImageData();
    const bytes = await processor(imageData, imageType, imageQuality);
    return new Blob([toArrayBuffer(bytes)], { type: imageType });
  };
}

/**
 * 自定义图像编码器（返回 Buffer）
 *
 * 与 createCustomImageDataToBlobConverter 类似，但直接返回 Buffer
 * 而不包装为 Blob，适用于不需要 Blob 的 Node.js 环境。
 *
 * @param processor - 自定义图像处理函数
 * @returns ImageDataConverter<Buffer> 转换器实例（输出类型为 Buffer）
 */
export function createCustomImageDataToBufferConverter(
  processor: (
    imageData: PdfImage,
    imageType: ImageConversionTypes,
    imageQuality?: number,
  ) => Promise<Buffer>,
): ImageDataConverter<Buffer> {
  return async (
    getImageData: LazyImageData,
    imageType: ImageConversionTypes = 'image/webp',
    imageQuality?: number,
  ): Promise<Buffer> => {
    const imageData = getImageData();
    return await processor(imageData, imageType, imageQuality);
  };
}
