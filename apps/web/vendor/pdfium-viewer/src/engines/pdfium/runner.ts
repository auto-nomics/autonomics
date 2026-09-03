/**
 * @file runner.ts
 * @description PDFium WASM 引擎运行器
 *
 * 本文件定义了 PdfiumEngineRunner 类，它是基于 PDFium WASM 的 PDF 引擎运行器。
 * 运行器负责在 Web Worker 环境中初始化 PDFium WASM 模块，并创建 PdfiumNative 实例
 * 作为实际的 PDF 操作执行器。
 *
 * 继承关系：
 *   PdfiumEngineRunner → PdfiumNativeRunner（抽象基类，提供消息处理和生命周期管理）
 *
 * 初始化流程：
 *   1. 接收 WASM 二进制数据
 *   2. 通过 init() 加载并编译 WASM 模块
 *   3. 用编译后的模块创建 PdfiumNative 实例
 *   4. 调用 ready() 通知基类初始化完成，开始处理消息
 */

import { init } from '../../pdfium';
import { PdfiumNativeRunner } from '../orchestrator/pdfium-native-runner';
import { PdfiumNative } from './engine';
import { Logger } from '../../models';
import type { FontFallbackConfig } from './font-fallback';

/**
 * 基于 PDFium WASM 的引擎运行器
 *
 * 继承自 PdfiumNativeRunner，在 Web Worker 中运行。
 * 负责：
 *   - 加载和初始化 PDFium WASM 模块
 *   - 创建 PdfiumNative 引擎实例（执行实际的 PDF 操作）
 *   - 管理字体回退配置（用于处理 PDF 中缺失的字体）
 *
 * 生命周期：
 *   构造 → prepare() → ready() → handle()（持续处理来自主线程的消息）
 */
export class PdfiumEngineRunner extends PdfiumNativeRunner {
  /** 字体回退配置，用于在 PDF 引用的字体不可用时提供替代方案 */
  private fontFallback?: FontFallbackConfig;

  /**
   * 创建 PdfiumEngineRunner 实例
   *
   * @param wasmBinary - PDFium WASM 二进制数据（通常通过 fetch 从 URL 获取）
   *                     包含完整的 PDFium 库，用于 PDF 解析、渲染、文本提取等操作
   * @param logger - 可选的日志记录器实例，用于输出调试和错误信息
   * @param fontFallback - 可选的字体回退配置，定义当 PDF 中引用的字体缺失时
   *                       如何找到替代字体（通常使用 CDN 上的 Google Fonts）
   */
  constructor(
    private wasmBinary: ArrayBuffer,
    logger?: Logger,
    fontFallback?: FontFallbackConfig,
  ) {
    super(logger);
    this.fontFallback = fontFallback;
  }

  /**
   * 初始化运行器：加载 WASM 模块并创建引擎实例
   *
   * 执行步骤：
   *   1. 使用预加载的 WASM 二进制数据初始化 PDFium WASM 模块
   *   2. 创建 PdfiumNative 实例作为"哑执行器"（PdfiumNative 在构造函数中
   *      完成自身初始化，因此被称为"dumb"——它不处理消息，只执行具体操作）
   *   3. 调用 ready() 通知基类初始化完成：
   *      - 将 self.onmessage 重定向到 runner.handle()
   *      - 向主线程发送 'ready' 消息
   *
   * @throws 如果 WASM 编译失败或 PdfiumNative 初始化出错
   */
  async prepare() {
    const wasmBinary = this.wasmBinary;
    // 编译 WASM 二进制数据为可执行的模块实例
    const wasmModule = await init({ wasmBinary });

    // 创建 PdfiumNative 引擎实例（在构造函数中完成 PDFium 初始化）
    this.native = new PdfiumNative(wasmModule, {
      logger: this.logger,
      fontFallback: this.fontFallback,
    });

    // 通知基类初始化完成，切换到消息处理模式
    this.ready();
  }
}
