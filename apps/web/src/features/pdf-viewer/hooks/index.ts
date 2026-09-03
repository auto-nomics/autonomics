/**
 * PDF 查看器 - Hooks 模块入口
 *
 * 仅 re-export 给外部模块用的 hook。
 * WasmPdfViewer 内部直接 import 各 hook 文件，不依赖此 barrel。
 */

// 注释管理 hook
export { useAnnotations } from './useAnnotations';
