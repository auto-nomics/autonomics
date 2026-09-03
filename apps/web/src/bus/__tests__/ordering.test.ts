import { describe, it, expect } from 'vitest';
import {
  TYPE_ORDER,
  EVENT_ORDER,
  createSorter,
  sortTypes,
  sortEvents,
} from '../ordering';
import type { NotifierType, NotifierEvent } from '../types';

describe('ordering — 确定性排序', () => {
  describe('TYPE_ORDER', () => {
    it('paper 在 annotation 之前', () => {
      expect(TYPE_ORDER.indexOf('paper')).toBeLessThan(
        TYPE_ORDER.indexOf('annotation'),
      );
    });

    it('folder 在 paper 之前', () => {
      expect(TYPE_ORDER.indexOf('folder')).toBeLessThan(
        TYPE_ORDER.indexOf('paper'),
      );
    });

    it('包含所有 6 种类型', () => {
      expect(TYPE_ORDER).toHaveLength(6);
    });
  });

  describe('EVENT_ORDER', () => {
    it('add 在 modify 之前', () => {
      expect(EVENT_ORDER.indexOf('add')).toBeLessThan(
        EVENT_ORDER.indexOf('modify'),
      );
    });

    it('delete 在 trash 之前', () => {
      expect(EVENT_ORDER.indexOf('delete')).toBeLessThan(
        EVENT_ORDER.indexOf('trash'),
      );
    });

    it('包含所有 6 种事件', () => {
      expect(EVENT_ORDER).toHaveLength(6);
    });
  });

  describe('createSorter', () => {
    it('按已知顺序排序', () => {
      const sorter = createSorter<string>(['b', 'a', 'c']);
      expect(['c', 'a', 'b'].sort(sorter)).toEqual(['b', 'a', 'c']);
    });

    it('未知元素排在末尾', () => {
      const sorter = createSorter<string>(['a', 'b']);
      expect((['z', 'b', 'a'] as string[]).sort(sorter)).toEqual(['a', 'b', 'z']);
    });
  });

  describe('sortTypes', () => {
    it('按 TYPE_ORDER 排序', () => {
      const input: NotifierType[] = ['tag', 'paper', 'annotation'];
      expect(sortTypes(input)).toEqual(['paper', 'annotation', 'tag']);
    });

    it('空数组不变', () => {
      expect(sortTypes([])).toEqual([]);
    });
  });

  describe('sortEvents', () => {
    it('按 EVENT_ORDER 排序', () => {
      const input: NotifierEvent[] = ['delete', 'add', 'modify'];
      expect(sortEvents(input)).toEqual(['add', 'modify', 'delete']);
    });
  });
});
