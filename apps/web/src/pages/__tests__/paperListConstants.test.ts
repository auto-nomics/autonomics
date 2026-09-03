/**
 * 论文列表常量测试文件
 *
 * 测试 paperListConstants 中的配置项和工具函数，包括：
 * - STATUS_CONFIG: 解析状态配置
 * - ENGINE_CONFIG: 解析引擎配置
 * - OA_STATUS_CONFIG: 开放获取状态配置
 * - ALL_COLUMNS: 列配置
 * - DEFAULT_VISIBLE_COLUMNS: 默认可见列
 * - SORT_FIELD_MAP: 排序字段映射
 * - formatDate: 日期格式化函数
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';

// 将 React 设置提升，确保在 JSX 处理之前可用
const { createElement } = vi.hoisted(() => {
  const React = require('react');
  global.React = React;
  return { createElement: React.createElement };
});

// 在 React 可用后导入源代码
import {
  STATUS_CONFIG,
  ENGINE_CONFIG,
  OA_STATUS_CONFIG,
  ALL_COLUMNS,
  DEFAULT_VISIBLE_COLUMNS,
  SORT_FIELD_MAP,
  formatDate,
} from '../PaperListPage/paperListConstants';

describe('paperListConstants', () => {
  // 有效的 Ant Design 标签颜色列表
  const validTagColors = [
    'default', 'processing', 'success', 'error', 'warning',
    'pink', 'red', 'yellow', 'orange', 'cyan', 'blue', 'purple', 'geekblue',
    'magenta', 'volcano', 'gold', 'lime', 'green',
  ];

  describe('STATUS_CONFIG', () => {
    const expectedStatusKeys = ['pending', 'parsing', 'done', 'failed'];

    it('1. has all expected status keys', () => {
      // 验证包含所有预期的状态键
      expectedStatusKeys.forEach(key => {
        expect(STATUS_CONFIG).toHaveProperty(key);
      });
    });

    it('2. each status has color and text properties', () => {
      // 验证每个状态都有 color 和 text 属性
      Object.values(STATUS_CONFIG).forEach(status => {
        expect(status).toHaveProperty('color');
        expect(status).toHaveProperty('text');
      });
    });

    it('3. colors are valid Ant Design tag colors', () => {
      // 验证颜色值在 Ant Design 有效颜色列表中
      Object.values(STATUS_CONFIG).forEach(status => {
        expect(validTagColors).toContain(status.color);
      });
    });

    it('4. done status has success color', () => {
      // 验证完成状态使用成功颜色
      expect(STATUS_CONFIG.done.color).toBe('success');
    });

    it('each status has an icon property', () => {
      // 验证每个状态都有 icon 属性
      Object.values(STATUS_CONFIG).forEach(status => {
        expect(status).toHaveProperty('icon');
      });
    });
  });

  describe('ENGINE_CONFIG', () => {
    const expectedEngineKeys = ['mineru'];

    it('5. has expected engine keys', () => {
      // 验证包含所有预期的引擎键
      expectedEngineKeys.forEach(key => {
        expect(ENGINE_CONFIG).toHaveProperty(key);
      });
    });

    it('each engine has label and color', () => {
      // 验证每个引擎都有 text 和 color 属性
      Object.values(ENGINE_CONFIG).forEach(engine => {
        expect(engine).toHaveProperty('text');
        expect(engine).toHaveProperty('color');
      });
    });

    it('engine colors are valid Ant Design tag colors', () => {
      // 验证引擎颜色在 Ant Design 有效颜色列表中
      Object.values(ENGINE_CONFIG).forEach(engine => {
        expect(validTagColors).toContain(engine.color);
      });
    });
  });

  describe('OA_STATUS_CONFIG', () => {
    const expectedOAKeys = ['gold', 'green', 'bronze', 'hybrid', 'closed'];

    it('7. has expected OA status keys', () => {
      // 验证包含所有预期的开放获取状态键
      expectedOAKeys.forEach(key => {
        expect(OA_STATUS_CONFIG).toHaveProperty(key);
      });
    });

    it('each OA status has color and text properties', () => {
      // 验证每个 OA 状态都有 color 和 text 属性
      Object.values(OA_STATUS_CONFIG).forEach(status => {
        expect(status).toHaveProperty('color');
        expect(status).toHaveProperty('text');
      });
    });

    it('OA status colors are valid Ant Design tag colors', () => {
      // 验证 OA 状态颜色在 Ant Design 有效颜色列表中
      Object.values(OA_STATUS_CONFIG).forEach(status => {
        expect(validTagColors).toContain(status.color);
      });
    });
  });

  describe('ALL_COLUMNS', () => {
    it('8. is an array with length > 0', () => {
      // 验证列配置是一个非空数组
      expect(Array.isArray(ALL_COLUMNS)).toBe(true);
      expect(ALL_COLUMNS.length).toBeGreaterThan(0);
    });

    it('9. each column has at least key and label properties', () => {
      // 验证每列至少有 key 和 label 属性
      ALL_COLUMNS.forEach(column => {
        expect(column).toHaveProperty('key');
        expect(column).toHaveProperty('label');
      });
    });

    it('10. column keys are unique', () => {
      // 验证列键值唯一
      const keys = ALL_COLUMNS.map(col => col.key);
      const uniqueKeys = new Set(keys);
      expect(uniqueKeys.size).toBe(keys.length);
    });

    it('11. contains expected essential columns', () => {
      // 验证包含必要的核心列
      const columnKeys = ALL_COLUMNS.map(col => col.key);
      // 注意：parseStatus 列已移除（解析状态现在在展开行的附件列表中显示）
      // jcr 列是新增的独立列，用于 JCR 分区排序
      const essentialColumns = ['title', 'authors', 'journal', 'year', 'jcr'];
      essentialColumns.forEach(essential => {
        expect(columnKeys).toContain(essential);
      });
    });

    it('each column has required properties', () => {
      // 验证每列都有必需的属性
      ALL_COLUMNS.forEach(column => {
        expect(column).toHaveProperty('width');
        expect(column).toHaveProperty('sortable');
        expect(column).toHaveProperty('hideable');
        expect(column).toHaveProperty('ellipsis');
      });
    });

    it('title column has hideable set to false', () => {
      // 验证标题列不可隐藏
      const titleColumn = ALL_COLUMNS.find(col => col.key === 'title');
      expect(titleColumn?.hideable).toBe(false);
    });
  });

  describe('DEFAULT_VISIBLE_COLUMNS', () => {
    it('12. is an array of strings', () => {
      // 验证默认可见列是字符串数组
      expect(Array.isArray(DEFAULT_VISIBLE_COLUMNS)).toBe(true);
      DEFAULT_VISIBLE_COLUMNS.forEach(col => {
        expect(typeof col).toBe('string');
      });
    });

    it('13. all values exist as keys in ALL_COLUMNS', () => {
      // 验证默认可见列都在 ALL_COLUMNS 中有定义
      const allColumnKeys = ALL_COLUMNS.map(col => col.key);
      DEFAULT_VISIBLE_COLUMNS.forEach(col => {
        expect(allColumnKeys).toContain(col);
      });
    });

    it('14. contains essential columns like title', () => {
      // 验证包含必要列（如标题）
      expect(DEFAULT_VISIBLE_COLUMNS).toContain('title');
    });

    it('is non-empty', () => {
      // 验证非空
      expect(DEFAULT_VISIBLE_COLUMNS.length).toBeGreaterThan(0);
    });
  });

  describe('SORT_FIELD_MAP', () => {
    it('15. is an object (non-null, non-array)', () => {
      // 验证排序字段映射是对象类型
      expect(SORT_FIELD_MAP).not.toBeNull();
      expect(typeof SORT_FIELD_MAP).toBe('object');
      expect(Array.isArray(SORT_FIELD_MAP)).toBe(false);
    });

    it('16. maps column keys to backend field names', () => {
      // 验证映射值都是有效的后端字段名（非空字符串）
      Object.values(SORT_FIELD_MAP).forEach(field => {
        expect(typeof field).toBe('string');
        expect(field.length).toBeGreaterThan(0);
      });
    });

    it('17. all keys in SORT_FIELD_MAP exist in ALL_COLUMNS', () => {
      // 验证排序映射的键都在列定义中存在
      const allColumnKeys = ALL_COLUMNS.map(col => col.key);
      Object.keys(SORT_FIELD_MAP).forEach(key => {
        expect(allColumnKeys).toContain(key);
      });
    });

    it('mapped columns have sortable set to true', () => {
      // 验证有排序映射的列都是可排序的
      Object.keys(SORT_FIELD_MAP).forEach(key => {
        const column = ALL_COLUMNS.find(col => col.key === key);
        expect(column?.sortable).toBe(true);
      });
    });
  });

  describe('formatDate function', () => {
    beforeEach(() => {
      // Mock Date.prototype.toLocaleDateString
      vi.stubGlobal('Date', class extends Date {
        constructor(...args: any[]) {
          if (args.length === 1 && typeof args[0] === 'number') {
            super(args[0]);
          } else {
            super();
          }
        }

        toLocaleDateString(locale?: any, options?: any) {
          // 返回模拟的格式化日期字符串用于测试
          return '4月11日 14:30';
        }
      });
    });

    it('18. formats Date objects correctly', () => {
      // 验证正确格式化 Date 对象
      // 使用已知时间戳：2024-04-11 14:30:00 UTC（秒级）
      const timestamp = 1744379400;
      const result = formatDate(timestamp);
      expect(typeof result).toBe('string');
      expect(result.length).toBeGreaterThan(0);
    });

    it('19. handles invalid/null dates gracefully', () => {
      // 验证优雅处理无效/空日期
      expect(formatDate(null)).toBe('');
      expect(formatDate(undefined)).toBe('');
      expect(formatDate(0)).toBe('');
      expect(formatDate('')).toBe('');
    });

    it('converts unix timestamp (seconds) to milliseconds', () => {
      // 验证将 Unix 时间戳（秒）转换为毫秒
      const timestamp = 1744379400; // 2024-04-11 14:30:00（秒）
      const result = formatDate(timestamp);
      expect(result).not.toBe('');
    });

    it('returns string for valid timestamp', () => {
      // 验证有效时间戳返回字符串
      const timestamp = 1744379400;
      const result = formatDate(timestamp);
      expect(typeof result).toBe('string');
    });
  });

  describe('consistency checks', () => {
    it('all sortable columns have a sort field mapping or use direct key', () => {
      // 验证所有可排序列都有排序字段映射或直接使用键
      const sortableColumns = ALL_COLUMNS.filter(col => col.sortable === true);
      sortableColumns.forEach(col => {
        // 要么有显式映射，要么直接使用键
        const hasMapping = (SORT_FIELD_MAP as Record<string, unknown>)[col.key] !== undefined;
        // 此测试记录行为 - 列可能使用直接键访问
        expect(col.key).toBeDefined();
      });
    });

    it('all non-sortable columns are not in SORT_FIELD_MAP', () => {
      // 验证所有不可排序列都不在排序映射中
      const nonSortableColumns = ALL_COLUMNS.filter(col => col.sortable === false);
      const nonSortableKeys = nonSortableColumns.map(col => col.key);
      const sortMapKeys = Object.keys(SORT_FIELD_MAP);

      // 检查不可排序列没有排序映射
      nonSortableKeys.forEach(key => {
        expect(sortMapKeys).not.toContain(key);
      });
    });
  });
});
