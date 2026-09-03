/**
 * 论文列表页面共享上下文
 *
 * 提供基于 ref 的可变配置对象，供各个 hook 读取使用。
 * 使用 ref 可以让在 Provider 之后定义的回调函数仍然能够访问配置。
 */

import { createContext, useContext, useRef, useMemo } from 'react';

interface PaperListContextValue {
  configRef: React.MutableRefObject<any>;
}

// 创建上下文对象，初始值为 null
const PaperListContext = createContext<PaperListContextValue | null>(null);

/**
 * Provider 组件，持有共享的论文列表配置
 */
export function PaperListProvider({ children }: { children: React.ReactNode }) {
  // 使用 ref 存储可变配置 - 允许在 Provider 之后定义的回调函数正常工作
  const configRef = useRef<any>({});

  // 使用 useMemo 缓存 context 值，避免不必要的重渲染
  const value = useMemo(() => ({ configRef }), []);

  return (
    <PaperListContext.Provider value={value}>
      {children}
    </PaperListContext.Provider>
  );
}

/**
 * 用于访问论文列表配置的 Hook
 */
export function usePaperListConfig() {
  const context = useContext(PaperListContext);
  if (!context) {
    throw new Error('usePaperListConfig 必须在 PaperListProvider 内部使用');
  }
  return context.configRef;
}

export default PaperListContext;
