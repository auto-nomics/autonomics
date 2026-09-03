/**
 * @file 直接模式 PDFium 引擎工厂 — 在主线程中运行 PDFium
 *
 * 整体作用：
 *   本文件导出 createPdfiumEngine 工厂函数，用于创建一个在主线程（main thread）
 *   直接运行的 PDFium 引擎实例。这是 PDFium 引擎的两种运行模式之一，
 *   另一种是 Web Worker 模式（见 worker-engine.ts）。
 *
 * 架构概览：
 *   createPdfiumEngine 构建的引擎由两层组成：
 *     1. 底层执行器（PdfiumNative）—"笨"执行器，负责直接调用 PDFium WASM API
 *        进行文档加载、页面渲染等操作。在构造时初始化 PDFium 实例。
 *     2. 上层编排器（PdfEngine）—"聪明"编排器，负责：
 *        - 基于优先级的任务调度（如：页面渲染优先于缩略图生成）
 *        - 图像格式转换（将 ImageData 转为 Blob）
 *        - 生命周期管理和错误处理
 *
 * 适用场景：
 *   - 不支持 Web Worker 的环境
 *   - 简单的单页面预览
 *   - 调试和开发阶段（堆栈信息更直观）
 *
 * 注意事项：
 *   - 直接模式下 PDFium 的 CPU 密集型操作（如页面渲染）会阻塞主线程，
 *     对于大型 PDF 或需要流畅交互的场景，建议使用 worker-engine.ts 中的
 *     Worker 模式。
 *   - 图像编码器池（encoderPoolSize）默认为 0（禁用），因为主线程模式下
 *     创建 Worker 进行编码的意义不大；但也可以启用以获得并行编码能力。
 */

import { Logger } from '../../../models';
import { init } from '../../../pdfium';
import { PdfiumNative } from '../engine';
import { PdfEngine } from '../../orchestrator/pdf-engine';
import { browserImageDataToBlobConverter } from '../../converters/browser';
import type { FontFallbackConfig } from '../font-fallback';

export type { FontFallbackConfig };

/**
 * 创建直接模式 PDFium 引擎的配置选项
 */
export interface CreatePdfiumEngineOptions {
  /**
   * 日志记录器实例，用于输出调试和警告信息。
   * 如果不提供，引擎内部会使用静默日志器。
   */
  logger?: Logger;

  /**
   * 图像编码器 Worker 池的大小（默认值：0，即禁用）
   *
   * 图像编码器池的作用是将渲染后的 ImageData 并行编码为 Blob（PNG/JPEG），
   * 从而避免在主线程上执行耗时的编码操作。
   * 推荐设置为 2-4 个 Worker 以获得最佳并行编码性能。
   * 设为 0 时，编码将在主线程同步执行。
   */
  encoderPoolSize?: number;

  /**
   * 字体回退配置 — 处理 PDF 中缺失字体的策略
   *
   * 当 PDF 中引用了未嵌入的字体时，PDFium 会通过此配置请求回退字体。
   * 如果不配置，缺失字体的文本可能无法正确显示。
   */
  fontFallback?: FontFallbackConfig;
}

/**
 * 创建直接在主线程运行的 PDFium 引擎
 *
 * 此函数执行以下步骤：
 *   1. 通过 fetch 下载 pdfium.wasm 二进制文件
 *   2. 使用下载的 ArrayBuffer 初始化 PDFium WASM 模块
 *   3. 创建 PdfiumNative 执行器（封装 PDFium API 调用）
 *   4. 创建 PdfEngine 编排器（负责任务调度和图像转换）
 *
 * 注意：此函数是异步的，因为 WASM 模块的编译和初始化需要时间。
 *
 * @param wasmUrl - pdfium.wasm 文件的 URL 地址（可以是相对路径或绝对 URL）
 * @param options - 引擎配置选项（日志器、编码器池大小、字体回退等）
 * @returns Promise<PdfEngine<Blob>> - 已初始化的 PDF 引擎实例，输出类型为 Blob
 *
 * @example
 * // 基本用法 — 仅传入 WASM 路径
 * const engine = await createPdfiumEngine('/wasm/pdfium.wasm', { logger });
 *
 * @example
 * // 启用编码器池以获得并行图像编码能力
 * const engine = await createPdfiumEngine('/wasm/pdfium.wasm', {
 *   logger,
 *   encoderPoolSize: 2
 * });
 */
export async function createPdfiumEngine(
  wasmUrl: string,
  options?: CreatePdfiumEngineOptions,
): Promise<PdfEngine<Blob>> {
  // 第一步：通过 HTTP 请求获取 WASM 二进制文件
  // 使用 fetch + arrayBuffer 而非直接的 WebAssembly.instantiateStreaming，
  // 是因为 init 函数内部需要完整的 ArrayBuffer 来进行自定义初始化
  const response = await fetch(wasmUrl);
  const wasmBinary = await response.arrayBuffer();

  // 第二步：使用获取的二进制数据初始化 PDFium WASM 模块
  // init 函数会编译 WASM 模块并创建必要的绑定
  const wasmModule = await init({ wasmBinary });

  // 第三步：创建底层 "笨" 执行器
  // PdfiumNative 在构造函数中初始化 PDFium 引擎（FPDF_InitLibrary 等），
  // 并提供文档加载、页面渲染等基础 API 的封装
  const native = new PdfiumNative(wasmModule, {
    logger: options?.logger,
    fontFallback: options?.fontFallback,
  });

  // 第四步：创建上层 "聪明" 编排器
  // PdfEngine 接收执行器和配置，负责任务调度、优先级管理、图像转换等
  // browserImageDataToBlobConverter: 浏览器端的 ImageData -> Blob 转换器
  //   使用 Canvas API 进行编码，适用于直接模式和 Worker 模式的回退
  return new PdfEngine<Blob>(native, {
    imageConverter: browserImageDataToBlobConverter,
    logger: options?.logger,
  });
}
