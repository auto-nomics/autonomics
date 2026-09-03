/**
 * @file 引擎编排层（Orchestrator）入口
 *
 * 整体作用：
 *   编排层是 PDF 引擎的"智能"中间层，位于底层"哑"执行器
 *   （PdfiumNative 或 RemoteExecutor）和上层业务逻辑之间。
 *   它提供以下核心能力：
 *     - 基于优先级的任务调度（Priority-based task scheduling）
 *     - 可见性感知的任务排序（Visibility-aware task ranking）
 *     - 并行图像编码（Parallel image encoding）
 *     - 多页操作的编排（Multi-page operation orchestration）
 *
 * 架构层次：
 *   上层业务 → PdfEngine（编排层）→ IPdfiumExecutor（执行器）
 *                                  ↙            ↘
 *                         PdfiumNative      RemoteExecutor
 *                         （主线程直接）     （Worker 线程代理）
 *
 * @packageDocumentation
 */

export * from './task-queue';
export * from './pdf-engine';
export * from './remote-executor';
export * from './pdfium-native-runner';
