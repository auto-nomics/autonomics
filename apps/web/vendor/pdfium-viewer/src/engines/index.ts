/**
 * @file 引擎层（Engines）入口
 *
 * 导出所有可用的 PDF 引擎实现：
 *   - pdfium：基于 PDFium WASM 的引擎（支持主线程和 Worker 模式）
 *   - webworker：通用 Web Worker 引擎封装
 *
 * @packageDocumentation
 */

export * from './pdfium';
export * from './webworker';
