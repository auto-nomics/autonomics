import { describe, it, expect, vi, beforeEach } from 'vitest';
import { TransactionNotifier } from '../transactionNotifier';
import type { NotifierObserver } from '../types';

/**
 * 创建一个 mock 观察者，记录所有收到的通知。
 */
function createMockObserver(name = 'mock'): {
  observer: NotifierObserver;
  calls: Array<{
    event: string;
    type: string;
    ids: string[];
    extraData: Record<string, unknown>;
  }>;
} {
  const calls: Array<{
    event: string;
    type: string;
    ids: string[];
    extraData: Record<string, unknown>;
  }> = [];

  const observer: NotifierObserver = {
    notify: vi.fn(async (event, type, ids, extraData) => {
      calls.push({ event, type, ids, extraData });
    }),
  };

  return { observer, calls };
}

describe('TransactionNotifier — 核心事务通知器', () => {
  let notifier: TransactionNotifier;

  beforeEach(() => {
    notifier = new TransactionNotifier();
  });

  // ========== 基础事务流程 ==========

  describe('begin / trigger / commit 基础流程', () => {
    it('事务内 trigger 入队，commit 时统一通知', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      const txId = notifier.begin();
      notifier.trigger('add', 'paper', ['p1', 'p2']);
      notifier.trigger('modify', 'annotation', ['a1']);

      // 事务内不应有通知
      expect(calls).toHaveLength(0);

      await notifier.commit(txId);

      // commit 后按类型顺序通知：paper → annotation
      expect(calls).toHaveLength(2);
      expect(calls[0].type).toBe('paper');
      expect(calls[0].event).toBe('add');
      expect(calls[0].ids).toEqual(['p1', 'p2']);

      expect(calls[1].type).toBe('annotation');
      expect(calls[1].event).toBe('modify');
      expect(calls[1].ids).toEqual(['a1']);
    });

    it('事务外 trigger 直接通知观察者', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      await notifier.trigger('add', 'paper', ['p1']);

      expect(calls).toHaveLength(1);
      expect(calls[0].event).toBe('add');
      expect(calls[0].ids).toEqual(['p1']);
    });

    it('commit 后自动重置事务状态', async () => {
      notifier.begin();
      expect(notifier.inTransaction).toBe(true);

      await notifier.commit();

      expect(notifier.inTransaction).toBe(false);
    });
  });

  // ========== 事务回滚 ==========

  describe('reset — 回滚事务', () => {
    it('reset 清空队列，不触发通知', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      notifier.begin();
      notifier.trigger('add', 'paper', ['p1']);
      notifier.reset();

      expect(notifier.inTransaction).toBe(false);
      expect(calls).toHaveLength(0);
    });
  });

  // ========== 去重合并 ==========

  describe('事务内去重', () => {
    it('同一对象多次 modify 合并为一次通知', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      notifier.begin();
      notifier.trigger('modify', 'paper', ['p1']);
      notifier.trigger('modify', 'paper', ['p1']);
      notifier.trigger('modify', 'paper', ['p1']);
      await notifier.commit();

      // 多次 modify 合并后去重，只有一个 modify 通知
      const modifyCalls = calls.filter((c) => c.event === 'modify');
      expect(modifyCalls).toHaveLength(1);
      expect(modifyCalls[0].ids).toEqual(['p1']);
    });

    it('add + modify 同一对象合并为 add', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      notifier.begin();
      notifier.trigger('add', 'paper', ['p1']);
      notifier.trigger('modify', 'paper', ['p1']);
      await notifier.commit();

      expect(calls).toHaveLength(1);
      expect(calls[0].event).toBe('add');
      expect(calls[0].ids).toEqual(['p1']);
    });
  });

  // ========== 无效事件过滤 ==========

  describe('add + delete 互消', () => {
    it('同一对象 add 后 delete → 无通知', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      notifier.begin();
      notifier.trigger('add', 'paper', ['p1']);
      notifier.trigger('delete', 'paper', ['p1']);
      await notifier.commit();

      expect(calls).toHaveLength(0);
    });
  });

  describe('modify + delete → 只保留 delete', () => {
    it('同一对象 modify 后 delete → 只有 delete 通知', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      notifier.begin();
      notifier.trigger('modify', 'paper', ['p1']);
      notifier.trigger('delete', 'paper', ['p1']);
      await notifier.commit();

      expect(calls).toHaveLength(1);
      expect(calls[0].event).toBe('delete');
    });
  });

  describe('delete + modify → 丢弃 modify', () => {
    it('已删除对象的 modify 被丢弃', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      notifier.begin();
      notifier.trigger('delete', 'paper', ['p1']);
      notifier.trigger('modify', 'paper', ['p1']);
      await notifier.commit();

      expect(calls).toHaveLength(1);
      expect(calls[0].event).toBe('delete');
    });
  });

  // ========== 确定性排序 ==========

  describe('确定性排序', () => {
    it('类型顺序：paper 在 annotation 之前', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      notifier.begin();
      // 先触发 annotation，再触发 paper
      notifier.trigger('add', 'annotation', ['a1']);
      notifier.trigger('add', 'paper', ['p1']);
      await notifier.commit();

      expect(calls[0].type).toBe('paper');
      expect(calls[1].type).toBe('annotation');
    });

    it('事件顺序：add 在 modify 之前', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      notifier.begin();
      notifier.trigger('modify', 'paper', ['p1']);
      notifier.trigger('add', 'paper', ['p2']);
      await notifier.commit();

      expect(calls[0].event).toBe('add');
      expect(calls[1].event).toBe('modify');
    });

    it('完整排序：类型优先，类型内事件优先', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      notifier.begin();
      // 乱序触发
      notifier.trigger('delete', 'annotation', ['a1']);
      notifier.trigger('modify', 'paper', ['p1']);
      notifier.trigger('add', 'paper', ['p2']);
      notifier.trigger('add', 'annotation', ['a2']);
      await notifier.commit();

      // paper 先于 annotation
      expect(calls[0].type).toBe('paper');
      expect(calls[0].event).toBe('add');
      expect(calls[1].type).toBe('paper');
      expect(calls[1].event).toBe('modify');
      expect(calls[2].type).toBe('annotation');
      expect(calls[2].event).toBe('add');
      expect(calls[3].type).toBe('annotation');
      expect(calls[3].event).toBe('delete');
    });
  });

  // ========== 观察者管理 ==========

  describe('registerObserver / unregisterObserver', () => {
    it('类型过滤：只接收注册的类型', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer, ['paper']);

      await notifier.trigger('add', 'paper', ['p1']);
      await notifier.trigger('add', 'annotation', ['a1']);

      expect(calls).toHaveLength(1);
      expect(calls[0].type).toBe('paper');
    });

    it('无类型过滤：接收所有类型', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      await notifier.trigger('add', 'paper', ['p1']);
      await notifier.trigger('add', 'annotation', ['a1']);

      expect(calls).toHaveLength(2);
    });

    it('unregisterObserver 后不再接收通知', async () => {
      const { observer, calls } = createMockObserver();
      const id = notifier.registerObserver(observer);

      await notifier.trigger('add', 'paper', ['p1']);
      expect(calls).toHaveLength(1);

      notifier.unregisterObserver(id);
      await notifier.trigger('add', 'paper', ['p2']);
      expect(calls).toHaveLength(1);
    });

    it('支持 options 对象参数', async () => {
      const { observer, calls } = createMockObserver();
      const id = notifier.registerObserver(observer, {
        types: ['paper', 'annotation'],
        priority: 10,
        id: 'myObserver',
      });

      await notifier.trigger('add', 'paper', ['p1']);
      await notifier.trigger('add', 'annotation', ['a1']);
      await notifier.trigger('add', 'tag', ['n1']);

      expect(calls).toHaveLength(2);
      expect(calls[0].type).toBe('paper');
      expect(calls[1].type).toBe('annotation');

      notifier.unregisterObserver(id);
    });
  });

  // ========== 观察者优先级 ==========

  describe('观察者优先级', () => {
    it('priority 数值小的先执行', async () => {
      const order: string[] = [];

      notifier.registerObserver(
        {
          notify: vi.fn(async () => {
            order.push('low');
          }),
        },
        { priority: 50 },
      );

      notifier.registerObserver(
        {
          notify: vi.fn(async () => {
            order.push('high');
          }),
        },
        { priority: 10 },
      );

      notifier.registerObserver(
        {
          notify: vi.fn(async () => {
            order.push('default');
          }),
        },
      );

      await notifier.trigger('add', 'paper', ['p1']);

      expect(order).toEqual(['high', 'low', 'default']);
    });
  });

  // ========== 容错 ==========

  describe('观察者容错', () => {
    it('一个观察者抛异常不影响其他观察者', async () => {
      const { observer: okObserver, calls: okCalls } = createMockObserver('ok');
      const errorObserver: NotifierObserver = {
        notify: vi.fn(async () => {
          throw new Error('boom');
        }),
      };

      notifier.registerObserver(errorObserver);
      notifier.registerObserver(okObserver);

      await notifier.trigger('add', 'paper', ['p1']);

      // ok 观察者应该仍然收到通知
      expect(okCalls).toHaveLength(1);
    });

    it('事务 commit 中一个观察者抛异常不影响其他', async () => {
      const { observer: okObserver, calls: okCalls } = createMockObserver('ok');
      const errorObserver: NotifierObserver = {
        notify: vi.fn(async () => {
          throw new Error('boom');
        }),
      };

      notifier.registerObserver(errorObserver);
      notifier.registerObserver(okObserver);

      notifier.begin();
      notifier.trigger('add', 'paper', ['p1']);
      await notifier.commit();

      expect(okCalls).toHaveLength(1);
    });
  });

  // ========== 存在性检查 ==========

  describe('setExistenceChecker', () => {
    it('通过 checker 过滤不存在对象的 modify', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      notifier.setExistenceChecker(async (_type, id) => id !== 'p2');

      notifier.begin();
      notifier.trigger('modify', 'paper', ['p1', 'p2', 'p3']);
      await notifier.commit();

      expect(calls).toHaveLength(1);
      expect(calls[0].ids).toEqual(['p1', 'p3']);
    });
  });

  // ========== 参数校验 ==========

  describe('参数校验', () => {
    it('无效类型抛异常', () => {
      expect(() =>
        notifier.trigger('add', 'invalid_type' as any, ['id1']),
      ).toThrow('Invalid type');
    });

    it('无效事件抛异常', () => {
      expect(() =>
        notifier.trigger('invalid_event' as any, 'paper', ['id1']),
      ).toThrow('Invalid event');
    });

    it('事务外 commit 抛异常', async () => {
      await expect(notifier.commit()).rejects.toThrow('commit() called outside');
    });
  });

  // ========== 边界情况 ==========

  describe('边界情况', () => {
    it('commit 空事务不通知', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      notifier.begin();
      await notifier.commit();

      expect(calls).toHaveLength(0);
    });

    it('mismatched transactionId 不执行', async () => {
      const { observer, calls } = createMockObserver();
      notifier.registerObserver(observer);

      const txId = notifier.begin();
      notifier.trigger('add', 'paper', ['p1']);

      // 传入错误的事务 ID
      await notifier.commit('wrong_id');

      // 不应通知（ID 不匹配）
      expect(calls).toHaveLength(0);

      // 清理
      notifier.reset(txId);
    });
  });
});
