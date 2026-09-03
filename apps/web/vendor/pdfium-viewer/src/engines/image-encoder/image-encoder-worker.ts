/**
 * @file 图像编码 Worker — 在独立线程中使用 OffscreenCanvas 编码图像
 *
 * 整体作用：
 *   将 OffscreenCanvas.convertToBlob() 操作从 PDFium Worker 线程卸载到
 *   独立的编码 Worker 线程中，避免编码操作阻塞 PDF 渲染。
 *
 * 通信协议：
 *   主线程 → Worker：EncodeImageRequest（包含像素数据和目标格式）
 *   Worker → 主线程：EncodeImageResponse（包含编码后的 Blob 或错误信息）
 *
 * 编码策略：
 *   - BMP：直接内存拼接（最快，无需 Canvas）
 *   - PNG/WebP/JPEG：使用 OffscreenCanvas.convertToBlob()
 */

import { rgbaToBmpBlob } from './bmp';

/**
 * 编码请求消息（主线程 → Worker）
 */
export interface EncodeImageRequest {
  /** 请求唯一标识 */
  id: string;
  /** 消息类型（固定为 'encode'） */
  type: 'encode';
  /** 编码参数 */
  data: {
    /** 原始像素数据 */
    imageData: {
      data: Uint8ClampedArray;
      width: number;
      height: number;
    };
    /** 目标图像格式 */
    imageType: 'image/png' | 'image/jpeg' | 'image/webp' | 'image/bmp';
    /** 编码质量（0-1，仅对有损格式有效） */
    quality?: number;
  };
}

/**
 * 编码响应消息（Worker → 主线程）
 */
export interface EncodeImageResponse {
  /** 对应请求的唯一标识 */
  id: string;
  /** 响应类型：'result'（成功）| 'error'（失败） */
  type: 'result' | 'error';
  /** 成功时为编码后的 Blob，失败时为错误信息对象 */
  data: Blob | { message: string };
}

/**
 * 核心编码函数 — 将 RGBA 像素数据编码为指定格式的 Blob
 *
 * @param imageData - 原始像素数据（RGBA 格式）
 * @param imageType - 目标格式
 * @param quality   - 编码质量（可选）
 * @returns 编码后的 Blob
 */
async function encodeImage(
  imageData: { data: Uint8ClampedArray; width: number; height: number },
  imageType: 'image/png' | 'image/jpeg' | 'image/webp' | 'image/bmp',
  quality?: number,
): Promise<Blob> {
  // BMP 快速路径：无需 Canvas，直接内存拼接
  if (imageType === 'image/bmp') {
    return rgbaToBmpBlob(imageData.data, imageData.width, imageData.height);
  }

  // 检查 OffscreenCanvas 是否可用
  if (typeof OffscreenCanvas === 'undefined') {
    throw new Error('OffscreenCanvas is not available in this worker environment');
  }

  const { data, width, height } = imageData;

  // 构造 ImageData 对象
  const imgData = new ImageData(new Uint8ClampedArray(data), width, height);

  // 使用 OffscreenCanvas 进行编码
  const canvas = new OffscreenCanvas(width, height);
  const ctx = canvas.getContext('2d');

  if (!ctx) {
    throw new Error('Failed to get 2D context from OffscreenCanvas');
  }

  // 将像素数据绘制到 Canvas 上
  ctx.putImageData(imgData, 0, 0);

  // 转换为目标格式的 Blob
  return canvas.convertToBlob({ type: imageType, quality });
}

/**
 * Worker 消息处理入口
 *
 * 接收编码请求，执行编码，返回结果或错误。
 */
self.onmessage = async (event: MessageEvent<EncodeImageRequest>) => {
  const request = event.data;

  if (request.type !== 'encode') {
    return;
  }

  try {
    const { imageData, imageType, quality } = request.data;

    // 执行编码
    const blob = await encodeImage(imageData, imageType, quality);

    // 返回成功响应
    const response: EncodeImageResponse = {
      id: request.id,
      type: 'result',
      data: blob,
    };

    self.postMessage(response);
  } catch (error) {
    // 返回错误响应
    const response: EncodeImageResponse = {
      id: request.id,
      type: 'error',
      data: {
        message: error instanceof Error ? error.message : String(error),
      },
    };

    self.postMessage(response);
  }
};
