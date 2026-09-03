/**
 * @file PDF 引擎 Context Provider 组件
 *
 * 提供一个 React Context Provider，将 PDF 引擎实例注入到组件树中。
 * 该组件是引擎无关的（engine-agnostic），接受任何实现了 PdfEngine 接口的实例，
 * 因此可以用于 PDFium 或其他 PDF 引擎。
 *
 * 通常与 usePdfiumEngine hook 配合使用：
 * ```tsx
 * const { engine, isLoading, error } = usePdfiumEngine();
 * return (
 *   <PdfEngineProvider engine={engine} isLoading={isLoading} error={error}>
 *     <App />
 *   </PdfEngineProvider>
 * );
 * ```
 */
import { ReactNode } from '../../react/adapter';
import { PdfEngineContext } from '../context';
import type { PdfEngine } from '../../../models';

/** PdfEngineProvider 的 Props 类型 */
export interface PdfEngineProviderProps {
  /** 子组件 */
  children: ReactNode;
  /** PDF 引擎实例（加载中或出错时为 null） */
  engine: PdfEngine | null;
  /** 引擎是否正在加载 */
  isLoading: boolean;
  /** 加载错误信息 */
  error: Error | null;
}

/**
 * PDF 引擎 Context Provider
 *
 * 将引擎实例及其加载状态注入到 React Context 中，
 * 使子组件可以通过 useEngineContext / useEngine 访问引擎。
 */
export function PdfEngineProvider({ children, engine, isLoading, error }: PdfEngineProviderProps) {
  const contextValue = {
    engine,
    isLoading,
    error,
  };

  return <PdfEngineContext.Provider value={contextValue}>{children}</PdfEngineContext.Provider>;
}
