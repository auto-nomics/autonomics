/**
 * JayRead Event Bus Store - React 集成层
 *
 * Zustand store 包装事件总线实例，使其可以通过 React 的 selector 模式访问。
 * 虽然事件总线本身是单例，但这个 store 提供：
 * 1. 与 React 生命周期集成的能力
 * 2. 调试工具支持（通过 Zustand DevTools）
 * 3. 组件卸载时自动清理订阅
 *
 * 与 bus/index.ts 的关系：
 * - bus/index.ts: 核心事件总线，独立的 pub/sub 系统
 * - useBusStore: React 专用封装，提供 hooks 和状态追踪
 *
 * 何时使用：
 * - 在 React 组件中订阅事件: 使用 useBusSubscription hook
 * - 在 React 组件中获取事件状态: 使用 useBusStore selector
 * - 在非 React 代码中: 直接使用 JayreadBus（从 bus/index.ts 导入）
 *
 * @example 订阅事件（React 组件）
 * ```tsx
 * import { useBusSubscription } from '@/stores/useBusStore';
 *
 * function MyComponent() {
 *   useBusSubscription('paper.opened', (payload) => {
 *     console.log('论文打开了:', payload.paperID);
 *   });
 * }
 * ```
 *
 * @example 获取事件状态（React 组件）
 * ```tsx
 * import useBusStore from '@/stores/useBusStore';
 *
 * function DebugPanel() {
 *   const { lastEvent, eventCount } = useBusStore();
 *   return <div>Events: {eventCount}, Last: {lastEvent?.event}</div>;
 * }
 * ```
 */

import * as React from 'react';
import { create } from 'zustand';
import { JayreadBus } from '../bus';

interface BusState {
  // 调试模式
  debug: boolean;
  // 最后一次事件的记录
  lastEvent: { event: string; timestamp: number } | null;
  // 事件历史
  eventCount: number;
}

// 内部引用，用于调试
let _lastEmitCount = 0;

const useBusStore = create<BusState>(() => ({
  debug: false,
  lastEvent: null,
  eventCount: 0,
}));

/**
 * 订阅事件 - 在 React 组件中订阅事件总线事件，组件卸载时自动退订
 *
 * @param eventName 事件名称
 * @param listener 事件处理函数
 *
 * @example
 * ```tsx
 * function MyComponent() {
 *   useBusSubscription('paper.opened', (payload) => {
 *     console.log('论文打开了:', payload.paperID);
 *   });
 * }
 * ```
 */
export function useBusSubscription(
  eventName: string,
  listener: (payload: unknown) => void
): void {
  const listenerRef = React.useRef(listener);
  listenerRef.current = listener;

  React.useEffect(() => {
    const unsubscribe = JayreadBus.on(eventName, (payload) => {
      listenerRef.current(payload);
      _lastEmitCount++;
      useBusStore.setState({
        lastEvent: { event: eventName, timestamp: Date.now() },
        eventCount: _lastEmitCount,
      });
    });

    return () => {
      unsubscribe();
    };
  }, [eventName]);
}

export default useBusStore;
