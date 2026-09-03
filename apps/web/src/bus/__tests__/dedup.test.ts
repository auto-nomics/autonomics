import { describe, it, expect } from 'vitest';
import {
  mergeEvent,
  deduplicateIds,
  compressEvents,
  filterInvalidEvents,
  queueSize,
} from '../dedup';
import type { EventQueue } from '../types';

describe('dedup — 事件去重、合并与过滤', () => {
  describe('mergeEvent', () => {
    it('向空队列添加新事件', () => {
      const queue: EventQueue = {};
      mergeEvent(queue, 'add', 'paper', ['p1'], { paperID: 'p1' });

      expect(queue.paper).toBeDefined();
      expect(queue.paper!.add).toBeDefined();
      expect(queue.paper!.add!.ids).toEqual(['p1']);
    });

    it('合并同 type+event 的 ids', () => {
      const queue: EventQueue = {};
      mergeEvent(queue, 'add', 'paper', ['p1']);
      mergeEvent(queue, 'add', 'paper', ['p2']);

      expect(queue.paper!.add!.ids).toEqual(['p1', 'p2']);
    });

    it('不同事件类型不合并', () => {
      const queue: EventQueue = {};
      mergeEvent(queue, 'add', 'paper', ['p1']);
      mergeEvent(queue, 'modify', 'paper', ['p1']);

      expect(queue.paper!.add!.ids).toEqual(['p1']);
      expect(queue.paper!.modify!.ids).toEqual(['p1']);
    });
  });

  describe('deduplicateIds', () => {
    it('去重同一 (type, event) 中的重复 id', () => {
      const queue: EventQueue = {
        paper: {
          modify: { ids: ['p1', 'p2', 'p1', 'p3', 'p2'], data: {} },
        },
      };

      deduplicateIds(queue);

      expect(queue.paper!.modify!.ids).toEqual(['p1', 'p2', 'p3']);
    });

    it('不改变无重复的队列', () => {
      const queue: EventQueue = {
        paper: {
          add: { ids: ['p1', 'p2'], data: {} },
        },
      };

      deduplicateIds(queue);

      expect(queue.paper!.add!.ids).toEqual(['p1', 'p2']);
    });
  });

  describe('compressEvents — 事件压缩规则', () => {
    it('add + delete → 全部取消', () => {
      const queue: EventQueue = {
        paper: {
          add: { ids: ['p1'], data: {} },
          delete: { ids: ['p1'], data: {} },
        },
      };

      const result = compressEvents(queue);

      // p1 的 add+delete 互消，结果为空
      expect(result.paper).toBeUndefined();
    });

    it('add + modify → 合并为 add', () => {
      const queue: EventQueue = {
        paper: {
          add: { ids: ['p1'], data: { p1: { title: 'A' } } },
          modify: { ids: ['p1'], data: { p1: { title: 'B' } } },
        },
      };

      const result = compressEvents(queue);

      expect(result.paper!.add!.ids).toEqual(['p1']);
      expect(result.paper!.modify).toBeUndefined();
    });

    it('modify + modify → 合并为单个 modify', () => {
      const queue: EventQueue = {
        paper: {
          modify: { ids: ['p1'], data: { p1: { a: 1 } } },
        },
      };
      // 模拟两次 modify
      mergeEvent(queue, 'modify', 'paper', ['p1'], { p1: { b: 2 } });

      const result = compressEvents(queue);

      expect(result.paper!.modify!.ids).toEqual(['p1']);
      // modify 只有单个条目（去重后）
      expect(result.paper!.modify).toBeDefined();
    });

    it('modify + delete → 只保留 delete', () => {
      const queue: EventQueue = {
        paper: {
          modify: { ids: ['p1'], data: {} },
          delete: { ids: ['p1'], data: {} },
        },
      };

      const result = compressEvents(queue);

      expect(result.paper!.delete!.ids).toEqual(['p1']);
      expect(result.paper!.modify).toBeUndefined();
    });

    it('delete + modify → 丢弃 modify', () => {
      const queue: EventQueue = {
        paper: {
          delete: { ids: ['p1'], data: {} },
          modify: { ids: ['p1'], data: {} },
        },
      };

      const result = compressEvents(queue);

      expect(result.paper!.delete!.ids).toEqual(['p1']);
      expect(result.paper!.modify).toBeUndefined();
    });

    it('delete + add → add（重新创建）', () => {
      const queue: EventQueue = {
        paper: {
          delete: { ids: ['p1'], data: {} },
          add: { ids: ['p1'], data: { p1: { title: 'New' } } },
        },
      };

      const result = compressEvents(queue);

      expect(result.paper!.add!.ids).toEqual(['p1']);
      expect(result.paper!.delete).toBeUndefined();
    });

    it('多 id 独立压缩', () => {
      const queue: EventQueue = {
        paper: {
          add: { ids: ['p1', 'p2'], data: {} },
          delete: { ids: ['p1'], data: {} },
          modify: { ids: ['p2'], data: {} },
        },
      };

      const result = compressEvents(queue);

      // p1: add+delete 互消
      // p2: add+modify → add
      expect(result.paper!.add!.ids).toEqual(['p2']);
      expect(result.paper!.delete).toBeUndefined();
    });

    it('空队列返回空', () => {
      expect(compressEvents({})).toEqual({});
    });
  });

  describe('filterInvalidEvents', () => {
    it('无 checker 时原样返回', async () => {
      const queue: EventQueue = {
        paper: {
          modify: { ids: ['p1'], data: {} },
        },
      };

      const result = await filterInvalidEvents(queue);

      expect(result.paper!.modify!.ids).toEqual(['p1']);
    });

    it('过滤不存在对象的 modify 事件', async () => {
      const queue: EventQueue = {
        paper: {
          modify: { ids: ['p1', 'p2', 'p3'], data: {} },
        },
      };

      const checker = async (_type: string, id: string) => id !== 'p2';

      const result = await filterInvalidEvents(queue, checker);

      expect(result.paper!.modify!.ids).toEqual(['p1', 'p3']);
    });

    it('不过滤非 modify 事件', async () => {
      const queue: EventQueue = {
        paper: {
          delete: { ids: ['p1', 'p2'], data: {} },
        },
      };

      const checker = async () => false;

      const result = await filterInvalidEvents(queue, checker);

      expect(result.paper!.delete!.ids).toEqual(['p1', 'p2']);
    });

    it('所有 modify 对象都不存在时移除整个条目', async () => {
      const queue: EventQueue = {
        paper: {
          modify: { ids: ['p1'], data: {} },
        },
      };

      const checker = async () => false;

      const result = await filterInvalidEvents(queue, checker);

      expect(result.paper?.modify).toBeUndefined();
    });
  });

  describe('queueSize', () => {
    it('计算队列中所有 id 的总数', () => {
      const queue: EventQueue = {
        paper: {
          add: { ids: ['p1', 'p2'], data: {} },
          modify: { ids: ['p3'], data: {} },
        },
        annotation: {
          delete: { ids: ['a1', 'a2'], data: {} },
        },
      };

      expect(queueSize(queue)).toBe(5);
    });

    it('空队列为 0', () => {
      expect(queueSize({})).toBe(0);
    });
  });
});
