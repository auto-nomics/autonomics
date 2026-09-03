/**
 * @file PDF 引擎 Context 访问 Hooks
 *
 * 提供两个 hooks 用于从 React Context 中获取 PDF 引擎实例：
 *   - useEngineContext：返回完整的 Context 状态（engine + isLoading + error）
 *   - useEngine：简化版，直接返回引擎实例，加载错误会抛出异常
 */
import { useContext } from '../../react/adapter';
import { PdfEngineContext, PdfEngineContextState } from '../context';

/**
 * 获取 PDF 引擎的完整 Context 状态
 *
 * 包含 engine、isLoading、error 三个字段，适合需要精细控制
 * 加载状态和错误处理的场景。
 *
 * @returns PdfEngineContextState 引擎上下文状态
 * @throws 在 PdfEngineProvider 外部调用时抛出错误
 */
export function useEngineContext(): PdfEngineContextState {
  const contextValue = useContext(PdfEngineContext);

  if (contextValue === undefined) {
    throw new Error('useEngineContext must be used within a PdfEngineProvider');
  }

  return contextValue;
}

/**
 * 获取 PDF 引擎实例（简化版）
 *
 * 直接返回引擎实例。如果初始化出错，会将错误抛出
 * 到最近的 ErrorBoundary。
 *
 * @returns PdfEngine 实例，加载中返回 null
 * @throws 初始化错误会通过 throw 向上传播
 */
export function useEngine() {
  const { engine, error } = useEngineContext();

  if (error) {
    throw error;
  }

  return engine;
}
