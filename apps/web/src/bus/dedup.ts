/**
 * 事件去重、合并与过滤
 *
 * 提供纯函数来处理事件队列的合并和压缩逻辑。
 * 设计参考 Zotero Notifier 的 _mergeEvent，适配 TypeScript 类型系统。
 */

import type {
  NotifierType,
  NotifierEvent,
  EventQueue,
  QueueEntry,
  ExistenceChecker,
} from './types';
import { VALID_EVENTS } from './types';
import { sortTypes } from './ordering';

/**
 * 向队列合并一条事件（就地修改 queue）。
 *
 * 如果 queue 中已存在同 (type, event) 条目，合并 ids 和 extraData；
 * 否则创建新条目。
 */
export function mergeEvent(
  queue: EventQueue,
  event: NotifierEvent,
  type: NotifierType,
  ids: string[],
  extraData?: Record<string, unknown>,
): void {
  if (!queue[type]) {
    queue[type] = {};
  }
  const typeQueue = queue[type]!;

  if (!typeQueue[event]) {
    typeQueue[event] = { ids: [], data: {} };
  }
  const entry = typeQueue[event]!;

  // 合并 ids
  entry.ids = entry.ids.concat(ids);

  // 合并 extraData
  if (extraData) {
    for (const key of Object.keys(extraData)) {
      // 单 id 时直接存储值；多 id 时按 id 为键存储
      if (ids.length === 1) {
        entry.data[ids[0]] =
          extraData[ids[0]] !== undefined ? extraData[ids[0]] : extraData[key];
      } else {
        for (const id of ids) {
          if (extraData[id] !== undefined) {
            entry.data[id] = extraData[id];
          }
        }
      }
    }
  }
}

/**
 * 对队列中每个 (type, event) 的 ids 去重。
 */
export function deduplicateIds(queue: EventQueue): EventQueue {
  for (const type of Object.keys(queue) as NotifierType[]) {
    const typeQueue = queue[type];
    if (!typeQueue) continue;

    for (const event of Object.keys(typeQueue) as NotifierEvent[]) {
      const entry = typeQueue[event];
      if (!entry) continue;

      entry.ids = [...new Set(entry.ids)];
    }
  }
  return queue;
}

/**
 * 压缩事件队列 —— 对同一 type+id 的事件序列应用合并规则。
 *
 * 规则：
 * - add + delete → 全部取消（对象从未对外可见）
 * - add + modify → 合并为 add
 * - modify + modify → 合并为单个 modify
 * - modify + delete → 只保留 delete
 * - delete + modify → 丢弃 modify（已删除对象的修改无效）
 * - delete + add → add（重新创建）
 *
 * 返回新的压缩后队列。
 */
export function compressEvents(queue: EventQueue): EventQueue {
  const result: EventQueue = {};

  // 追踪每个 (type, id) 的事件序列
  const idEventMap = new Map<
    string, // `${type}::${id}`
    { event: NotifierEvent; data: Record<string, unknown> }[]
  >();

  // 按类型确定性顺序遍历，但事件保持原始插入顺序
  // （事件的时间顺序对压缩至关重要：add→delete 取消，delete→add 重建）
  const types = sortTypes(Object.keys(queue) as NotifierType[]);
  for (const type of types) {
    const typeQueue = queue[type];
    if (!typeQueue) continue;

    // 不排序事件 — 保持原始插入顺序
    for (const event of Object.keys(typeQueue) as NotifierEvent[]) {
      const entry = typeQueue[event];
      if (!entry) continue;

      for (const id of entry.ids) {
        const key = `${type}::${id}`;
        if (!idEventMap.has(key)) {
          idEventMap.set(key, []);
        }
        idEventMap.get(key)!.push({
          event,
          data: entry.data[id] != null
            ? entry.data[id] as Record<string, unknown>
            : {},
        });
      }
    }
  }

  // 对每个 (type, id) 应用压缩规则
  for (const [key, eventSeq] of idEventMap) {
    const [type] = key.split('::') as [NotifierType, string];
    const compressed = compressSequence(eventSeq);

    for (const { event, data } of compressed) {
      if (!result[type]) {
        result[type] = {};
      }
      if (!result[type]![event]) {
        result[type]![event] = { ids: [], data: {} };
      }
      const entry = result[type]![event]!;
      const id = key.split('::')[1];
      entry.ids.push(id);
      if (Object.keys(data).length > 0) {
        entry.data[id] = data;
      }
    }
  }

  return result;
}

/**
 * 压缩单个 id 的事件序列。
 * 按事件出现顺序依次应用两两合并规则。
 */
function compressSequence(
  seq: { event: NotifierEvent; data: Record<string, unknown> }[],
): { event: NotifierEvent; data: Record<string, unknown> }[] {
  if (seq.length <= 1) return seq;

  const result: { event: NotifierEvent; data: Record<string, unknown> }[] = [];

  for (const item of seq) {
    if (result.length === 0) {
      result.push(item);
      continue;
    }

    const prev = result[result.length - 1];
    const merged = tryMerge(prev.event, item.event, prev.data, item.data);

    if (merged === null) {
      // 互消：移除前一个，不添加当前
      result.pop();
    } else if (merged.cancelPrev) {
      // 前一个被取代
      result.pop();
      result.push({ event: merged.event, data: merged.data });
    } else if (merged.replacePrev) {
      // 前一个被更新（事件类型不变）
      prev.data = merged.data;
    } else {
      // 无法合并，追加
      result.push(item);
    }
  }

  return result;
}

interface MergeResult {
  event: NotifierEvent;
  data: Record<string, unknown>;
  cancelPrev?: boolean;
  replacePrev?: boolean;
}

/**
 * 两两合并规则。
 * 返回 null 表示互消，否则返回合并结果。
 */
function tryMerge(
  prevEvent: NotifierEvent,
  currEvent: NotifierEvent,
  prevData: Record<string, unknown>,
  currData: Record<string, unknown>,
): MergeResult | null {
  const mergedData = { ...prevData, ...currData };

  switch (prevEvent) {
    case 'add':
      switch (currEvent) {
        case 'delete':
          return null; // add + delete 互消
        case 'modify':
          return { event: 'add', data: mergedData, replacePrev: true };
        default:
          break;
      }
      break;

    case 'modify':
      switch (currEvent) {
        case 'modify':
          return { event: 'modify', data: mergedData, replacePrev: true };
        case 'delete':
          return { event: 'delete', data: currData, cancelPrev: true };
        default:
          break;
      }
      break;

    case 'delete':
      switch (currEvent) {
        case 'modify':
          // 已删除对象的 modify 被丢弃
          return { event: 'delete', data: prevData, replacePrev: true };
        case 'add':
          return { event: 'add', data: currData, cancelPrev: true };
        default:
          break;
      }
      break;

    default:
      break;
  }

  // 无法合并
  return { event: currEvent, data: currData };
}

/**
 * 过滤无效事件：对已删除对象（在 delete/trash 之后出现）的 modify 事件静默丢弃。
 *
 * 注意：compressEvents 已处理大部分 delete+modify 情况。
 * 此函数提供额外的运行时过滤，可通过 existenceChecker 检查对象是否实际存在。
 *
 * @param queue 事件队列
 * @param existenceChecker 可选的存在性检查函数
 * @returns 过滤后的队列
 */
export async function filterInvalidEvents(
  queue: EventQueue,
  existenceChecker?: ExistenceChecker,
): Promise<EventQueue> {
  if (!existenceChecker) return queue;

  const result: EventQueue = {};

  for (const type of Object.keys(queue) as NotifierType[]) {
    const typeQueue = queue[type];
    if (!typeQueue) continue;

    for (const event of Object.keys(typeQueue) as NotifierEvent[]) {
      const entry = typeQueue[event];
      if (!entry) continue;

      if (event === 'modify' && existenceChecker) {
        // 过滤不存在的对象
        const validIds: string[] = [];
        for (const id of entry.ids) {
          const exists = await existenceChecker(type, id);
          if (exists) {
            validIds.push(id);
          }
        }
        if (validIds.length > 0) {
          if (!result[type]) result[type] = {};
          result[type]![event] = {
            ids: validIds,
            data: entry.data,
          };
        }
      } else {
        if (!result[type]) result[type] = {};
        result[type]![event] = entry;
      }
    }
  }

  return result;
}

/**
 * 获取队列中的事件总数（用于调试）。
 */
export function queueSize(queue: EventQueue): number {
  let count = 0;
  for (const type of Object.keys(queue)) {
    const typeQueue = queue[type as NotifierType];
    if (!typeQueue) continue;
    for (const event of Object.keys(typeQueue)) {
      const entry = typeQueue[event as NotifierEvent];
      if (entry) count += entry.ids.length;
    }
  }
  return count;
}
