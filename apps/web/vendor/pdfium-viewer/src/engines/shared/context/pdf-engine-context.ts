/**
 * @file PDF 引擎 React Context 定义
 *
 * 定义 PdfEngineContext，用于在整个组件树中共享 PDF 引擎实例。
 * Context 的值包含三个状态：
 *   - engine：已初始化的引擎实例（加载中或出错时为 null）
 *   - isLoading：引擎是否正在初始化
 *   - error：初始化过程中的错误信息
 */
import { createContext } from '../../react/adapter';
import type { PdfEngine } from '../../../models';

/** PDF 引擎 Context 的状态类型 */
export interface PdfEngineContextState {
  /** 引擎实例，加载完成前为 null */
  engine: PdfEngine | null;
  /** 是否正在加载/初始化引擎 */
  isLoading: boolean;
  /** 初始化错误，无错误时为 null */
  error: Error | null;
}

/**
 * PDF 引擎 Context
 *
 * 初始值为 undefined，用于检测是否在 Provider 外部使用了 Context。
 * 子组件通过 useEngineContext hook 安全地访问此 Context。
 */
export const PdfEngineContext = createContext<PdfEngineContextState | undefined>(undefined);
