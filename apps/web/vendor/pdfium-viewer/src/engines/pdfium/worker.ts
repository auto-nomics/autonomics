/**
 * @file worker.ts
 * @description PDFium Web Worker 入口点
 *
 * 本文件是 PDFium WASM 引擎的 Web Worker 入口。
 * 它在独立的 Worker 线程中运行，负责：
 *   - 监听来自主线程的初始化消息
 *   - 下载 WASM 二进制文件
 *   - 创建并初始化 PdfiumEngineRunner 实例
 *   - 初始化完成后，由 runner 接管后续消息处理
 *
 * 架构说明：
 *   主线程通过 postMessage 发送指令 → Worker 接收并转发给 PdfiumEngineRunner
 *   → PdfiumEngineRunner 委托 PdfiumNative 执行具体操作 → 结果通过 postMessage 返回主线程
 *
 * 消息协议：
 *   初始化消息：{ type: 'wasmInit', wasmUrl: string, logger?: ..., fontFallback?: ... }
 *   初始化成功：runner.prepare() 内部发送 { type: 'ready' }
 *   初始化失败：发送 { type: 'wasmError', error: string }
 */

import { deserializeLogger } from '../../models';
import { PdfiumEngineRunner } from './runner';
import type { FontFallbackConfig } from './font-fallback';
import { cdnFontConfig } from './cdn-fonts';

/** PDFium 引擎运行器实例，初始化后非空 */
let runner: PdfiumEngineRunner | null = null;

/**
 * Worker 消息监听器
 *
 * 监听来自主线程的消息。仅处理初始化消息（type === 'wasmInit'），
 * 初始化完成后，runner.prepare() 内部会将 self.onmessage 重定向到
 * runner.handle() 方法，后续消息由 runner 直接处理。
 *
 * @param event - 来自主线程的 MessageEvent
 * @param event.data.type - 消息类型，'wasmInit' 表示初始化请求
 * @param event.data.wasmUrl - WASM 文件的下载 URL
 * @param event.data.logger - 序列化的日志记录器（可通过 deserializeLogger 还原）
 * @param event.data.fontFallback - 字体回退配置，null 表示显式禁用，
 *                                  undefined 表示使用 CDN 默认配置
 */
self.onmessage = async (event: MessageEvent) => {
  const { type, wasmUrl, logger: serializedLogger, fontFallback } = event.data;

  if (type === 'wasmInit' && wasmUrl && !runner) {
    try {
      // 从提供的 URL 下载 WASM 二进制文件
      const response = await fetch(wasmUrl);
      const wasmBinary = await response.arrayBuffer();

      // 反序列化日志记录器（如果主线程传入了序列化的 logger）
      const logger = serializedLogger ? deserializeLogger(serializedLogger) : undefined;

      // 确定字体回退策略：
      //   - fontFallback === null → 用户显式禁用字体回退
      //   - fontFallback 为 FontFallbackConfig 对象 → 使用用户自定义配置
      //   - fontFallback === undefined → 使用 CDN 默认字体回退配置（浏览器环境）
      const effectiveFontFallback =
        fontFallback === null
          ? undefined // 显式禁用
          : ((fontFallback as FontFallbackConfig | undefined) ?? cdnFontConfig); // 使用 CDN 默认配置

      // 创建并初始化引擎运行器
      runner = new PdfiumEngineRunner(wasmBinary, logger, effectiveFontFallback);
      await runner.prepare();
      // runner.prepare() 调用 ready() 后会：
      // 1. 将 self.onmessage 设置为 runner.handle()
      // 2. 向主线程发送 'ready' 消息
    } catch (error) {
      // 初始化失败时向主线程报告错误
      const message = error instanceof Error ? error.message : String(error);
      self.postMessage({ type: 'wasmError', error: message });
    }
  }
};
