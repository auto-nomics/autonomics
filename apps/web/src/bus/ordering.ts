/**
 * 确定性排序 —— 保证跨模块通知顺序一致
 *
 * 类型优先级：library → folder → paper → annotation → tag → conversation
 * 事件优先级：add → modify → remove → move → delete → trash
 */

import type { NotifierType, NotifierEvent } from './types';

/** 类型通知顺序（索引越小越先通知） */
export const TYPE_ORDER: readonly NotifierType[] = [
  'library',
  'folder',
  'paper',
  'annotation',
  'tag',
  'conversation',
];

/** 事件通知顺序（同一类型内） */
export const EVENT_ORDER: readonly NotifierEvent[] = [
  'add',
  'modify',
  'remove',
  'move',
  'delete',
  'trash',
];

/**
 * 基于已知顺序数组创建排序比较器。
 * 未知元素排在末尾（保持原始相对顺序）。
 */
export function createSorter<T extends string>(
  orderArray: readonly T[],
): (a: T, b: T) => number {
  return (a, b) => {
    const posA = orderArray.indexOf(a);
    const posB = orderArray.indexOf(b);
    // 未知元素排到末尾
    const idxA = posA === -1 ? orderArray.length : posA;
    const idxB = posB === -1 ? orderArray.length : posB;
    return idxA - idxB;
  };
}

/** 对类型数组按 TYPE_ORDER 排序（原地排序并返回） */
export function sortTypes(types: NotifierType[]): NotifierType[] {
  return types.sort(createSorter(TYPE_ORDER));
}

/** 对事件数组按 EVENT_ORDER 排序（原地排序并返回） */
export function sortEvents(events: NotifierEvent[]): NotifierEvent[] {
  return events.sort(createSorter(EVENT_ORDER));
}
