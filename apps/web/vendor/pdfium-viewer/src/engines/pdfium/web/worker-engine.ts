/**
 * @file Worker 模式 PDFium 引擎工厂 — 在 Web Worker 中运行 PDFium
 *
 * 整体作用：
 *   本文件导出 createPdfiumEngine 工厂函数，用于创建一个在 Web Worker 中
 *   运行的 PDFium 引擎实例。这是推荐的生产环境运行模式，因为 PDFium 的
 *   页面渲染等操作是 CPU 密集型的，在独立线程中执行不会阻塞主线程。
 *
 * 架构概览：
 *   createPdfiumEngine 构建的引擎涉及三个层级的协作：
 *
 *   1. PdfEngine（编排器，主线程）
 *      - 负责任务调度、优先级管理、生命周期控制
 *      - 通过 RemoteExecutor 与 Worker 通信
 *
 *   2. RemoteExecutor（远程执行器，主线程）
 *      - 将 PdfEngine 的调用通过 postMessage 转发给 Worker
 *      - 处理 Worker 返回的结果和错误
 *
 *   3. Worker 线程（独立线程）
 *      - 加载并初始化 PDFium WASM 模块
 *      - 执行实际的文档加载、页面渲染等操作
 *      - 将渲染结果（原始像素数据）传回主线程
 *
 *   此外还有一个独立的图像编码器 Worker 池（ImageEncoderWorkerPool），
 *   负责将渲染得到的 ImageData 并行编码为 Blob（PNG/JPEG），
 *   进一步减少主线程的 CPU 占用。
 *
 * 与直接模式的区别：
 *   - 直接模式（direct-engine.ts）：PDFium 运行在主线程，简单但会阻塞 UI
 *   - Worker 模式（本文件）：PDFium 运行在独立线程，不阻塞 UI，推荐生产使用
 *
 * Worker 文件来源：
 *   通过 Vite 的 `?worker&url` 导入语法，由构建工具在打包时自动处理 Worker 脚本：
 *   - PdfiumWorkerUrl: PDFium Worker 脚本的 URL（包含 WASM 初始化和渲染逻辑）
 *   - EncoderWorkerUrl: 图像编码器 Worker 脚本的 URL（负责 ImageData -> Blob 转换）
 */

import { Logger } from '../../../models';
import { PdfEngine } from '../../orchestrator/pdf-engine';
import { RemoteExecutor } from '../../orchestrator/remote-executor';
import { ImageEncoderWorkerPool } from '../../image-encoder';
import { createHybridImageConverter } from '../../converters/browser';
import type { FontFallbackConfig } from '../font-fallback';

// Vite 的 Worker 导入语法 — 在构建/开发时由 Vite 打包器处理
// ?worker&url 参数告诉 Vite：
//   - worker: 将此文件作为 Worker 入口打包
//   - url: 返回打包后的 Worker 脚本 URL，而非 Worker 类
// @ts-ignore 是因为 TypeScript 不认识 Vite 的自定义查询参数
// @ts-ignore
import PdfiumWorkerUrl from '../../../engines/pdfium/worker.ts?worker&url';
// @ts-ignore
import EncoderWorkerUrl from '../../../engines/image-encoder/image-encoder-worker.ts?worker&url';

/** 重新导出 FontFallbackConfig 类型，方便外部引用 */
export type { FontFallbackConfig };

/**
 * 创建 Worker 模式 PDFium 引擎的配置选项
 */
export interface CreatePdfiumEngineOptions {
  /** 日志记录器实例，用于输出调试和警告信息 */
  logger?: Logger;

  /**
   * 图像编码器 Worker 池的大小
   *
   * 每个 Worker 负责将渲染得到的 ImageData 编码为 Blob。
   * 默认值为 2，即在后台有 2 个 Worker 并行处理图像编码任务。
   * 增大此值可提高多页面并发编码能力，但也会增加内存占用。
   */
  encoderPoolSize?: number;

  /** 字体回退配置，处理 PDF 中缺失字体的策略 */
  fontFallback?: FontFallbackConfig;
}

/**
 * 创建在 Web Worker 中运行的 PDFium 引擎
 *
 * 此函数执行以下步骤：
 *   1. 解析配置参数（支持直接传入 Logger 实例或完整配置对象）
 *   2. 创建 PDFium Worker 并通过 RemoteExecutor 建立通信通道
 *   3. 创建图像编码器 Worker 池用于并行图像编码
 *   4. 创建 PdfEngine 编排器，整合所有组件
 *
 * 注意：与直接模式不同，此函数是同步的（返回值不是 Promise），
 * 因为 Worker 的创建和通信通道的建立是同步操作。
 * WASM 模块的实际初始化在 Worker 线程中异步进行，
 * RemoteExecutor 会等待 Worker 就绪后再处理请求。
 *
 * @param wasmUrl - pdfium.wasm 文件的 URL，将传递给 Worker 进行加载
 * @param options - 配置选项，可以是 Logger 实例（向后兼容）或完整配置对象
 * @returns PdfEngine<Blob> - 已就绪的 PDF 引擎实例
 */
export function createPdfiumEngine(
  wasmUrl: string,
  options?: Logger | CreatePdfiumEngineOptions,
): PdfEngine<Blob> {
  // 兼容旧的 API 调用方式：options 可以直接是 Logger 实例
  // 通过检查对象是否包含 'debug' 属性来区分 Logger 和 CreatePdfiumEngineOptions
  // （Logger 类具有 debug 方法，因此 'debug' in options 为 true）
  const config: CreatePdfiumEngineOptions =
    options instanceof Object && 'debug' in options
      ? { logger: options as Logger }
      : (options as CreatePdfiumEngineOptions) || {};

  // 从统一配置中提取各参数
  const { logger, encoderPoolSize, fontFallback } = config;

  // 创建 PDFium Worker 实例
  // type: 'module' 表示 Worker 脚本使用 ES Module 语法（import/export），
  // 这是 Vite 打包的 Worker 所必需的
  const worker = new Worker(PdfiumWorkerUrl, { type: 'module' });

  // 创建远程执行器 — 作为主线程和 Worker 之间的桥梁
  // RemoteExecutor 将 PdfEngine 的调用通过 postMessage 发送给 Worker，
  // 并将 wasmUrl、logger、fontFallback 等配置传递给 Worker
  const remoteExecutor = new RemoteExecutor(worker, { wasmUrl, logger, fontFallback });

  // 创建图像编码器 Worker 池
  // encoderPoolSize 默认为 2，即在后台有 2 个 Worker 并行处理图像编码
  // 编码器 Worker 是独立于 PDFium Worker 的额外线程池，
  // 专门负责将 ImageData 转换为 Blob，避免阻塞 PDFium 的渲染线程
  const encoderPool = new ImageEncoderWorkerPool(
    encoderPoolSize ?? 2,
    EncoderWorkerUrl,
    logger,
  );

  // 创建 PdfEngine 编排器，整合远程执行器和混合图像转换器
  // createHybridImageConverter 创建一个"混合"转换器：
  //   - 优先使用编码器 Worker 池进行并行编码（高性能路径）
  //   - 如果 Worker 池不可用或编码失败，回退到主线程同步编码（兼容路径）
  return new PdfEngine<Blob>(remoteExecutor, {
    imageConverter: createHybridImageConverter(encoderPool),
    logger,
  });
}
