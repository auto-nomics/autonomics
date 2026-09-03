/**
 * @file PDFium 引擎自动初始化 Hook
 *
 * 提供一个 React Hook，自动完成 PDFium WASM 引擎的初始化生命周期：
 *   1. 根据 worker 参数选择 Worker 线程或主线程模式
 *   2. 动态导入对应的引擎工厂函数
 *   3. 创建引擎实例并通过 state 暴露给组件
 *   4. 组件卸载时自动关闭文档并销毁引擎
 *
 * 使用方式：
 * ```tsx
 * const { engine, isLoading, error } = usePdfiumEngine({
 *   wasmUrl: '/wasm/pdfium.wasm',
 *   worker: true,
 * });
 * ```
 */
import { useEffect, useRef, useState } from '../../react/adapter';
import { ignore, Logger, PdfEngine } from '../../../models';
import type { FontFallbackConfig } from '../..';

/** PDFium WASM 文件的默认路径 */
const defaultWasmUrl = `/wasm/pdfium.wasm`;

/** usePdfiumEngine 的配置参数 */
interface UsePdfiumEngineProps {
  /** WASM 文件的 URL，默认 '/wasm/pdfium.wasm' */
  wasmUrl?: string;
  /** 是否使用 Worker 线程运行引擎，默认 true */
  worker?: boolean;
  /** 日志记录器 */
  logger?: Logger;
  /** 图像编码 Worker 池大小 */
  encoderPoolSize?: number;
  /** 字体回退配置，用于处理 PDF 中缺失的字体 */
  fontFallback?: FontFallbackConfig;
}

/**
 * 自动初始化并管理 PDFium 引擎生命周期的 Hook
 *
 * 在组件挂载时创建引擎，卸载时自动销毁。
 * 支持切换 Worker/主线程模式，以及自定义 WASM 路径。
 *
 * @param config - 可选的引擎配置
 * @returns { engine, isLoading, error } 引擎实例及状态
 */
export function usePdfiumEngine(config?: UsePdfiumEngineProps) {
  const {
    wasmUrl = defaultWasmUrl,
    worker = true,
    logger,
    encoderPoolSize,
    fontFallback,
  } = config ?? {};

  // 引擎实例及加载状态
  const [engine, setEngine] = useState<PdfEngine | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<Error | null>(null);
  // 使用 ref 保存引擎引用，确保清理函数能访问到最新实例
  const engineRef = useRef<PdfEngine | null>(null);

  useEffect(() => {
    let cancelled = false;

    (async () => {
      try {
        // 根据配置选择 Worker 线程或主线程模式，动态导入对应的工厂函数
        const { createPdfiumEngine } = worker
          ? await import('../../pdfium/web/worker-engine')
          : await import('../../pdfium/web/direct-engine');

        // 创建引擎实例（加载 WASM、初始化 PDFium）
        const pdfEngine = await createPdfiumEngine(wasmUrl, {
          logger,
          encoderPoolSize,
          fontFallback,
        });
        engineRef.current = pdfEngine;
        setEngine(pdfEngine);
        setLoading(false);
      } catch (e) {
        if (!cancelled) {
          setError(e as Error);
          setLoading(false);
        }
      }
    })();

    // 清理函数：组件卸载时关闭所有文档并销毁引擎
    return () => {
      cancelled = true;
      engineRef.current?.closeAllDocuments?.().wait(() => {
        engineRef.current?.destroy?.();
        engineRef.current = null;
      }, ignore);
    };
  }, [wasmUrl, worker, logger, fontFallback]);

  return { engine, isLoading: loading, error };
}
