/**
 * PDF 查看器 - 注释模块入口
 *
 * 导出 AnnotationManager（高亮撤销/重做栈管理）。
 * TriggerEngine 已删除：实际触发逻辑由 useWasmHoverTranslation / useDoubleClickTerm 等 Hook 直接处理，
 * 抽象层没有被调用过。
 */

export { AnnotationManager } from './AnnotationManager';
