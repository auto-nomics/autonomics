/**
 * @file PDFium 引擎模块统一导出
 *
 * 作为 PDFium WASM 引擎的公共 API 入口，导出：
 *   - PdfiumNative 核心引擎类（engine）
 *   - WASM 辅助工具（helper）
 *   - 图像格式转换器（converters）
 *   - 引擎编排器（orchestrator）
 *   - 字体回退系统（font-fallback）
 *   - CDN 字体配置（cdn-fonts）
 *   - Web 端引擎工厂（direct-engine / worker-engine）
 */

export * from './engine';
export * from './helper';
export * from '../converters/types';
export * from '../converters/browser';
export * from '../orchestrator/pdf-engine';
export * from './font-fallback';
export * from './cdn-fonts';

// 导出 Web 端工厂函数（使用别名避免命名冲突）
export { createPdfiumEngine as createPdfiumDirectEngine } from './web/direct-engine';
export { createPdfiumEngine as createPdfiumWorkerEngine } from './web/worker-engine';
export type { CreatePdfiumEngineOptions } from './web/direct-engine';

/** PDFium WASM 文件的默认 CDN 地址（版本号在构建时替换） */
export const DEFAULT_PDFIUM_WASM_URL: string =
  'https://cdn.jsdelivr.net/npm/@embedpdf/pdfium@__PDFIUM_VERSION__/dist/pdfium.wasm';
