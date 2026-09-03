import { describe, it, expect, vi, beforeEach } from 'vitest';
import { TransactionNotifier } from '../transactionNotifier';

/**
 * 创建 mock bus（模拟 JayreadBus.emit）
 */
function createMockBus() {
  const emitted: Array<{ eventName: string; payload: Record<string, unknown> }> = [];
  return {
    emit: vi.fn((eventName: string, payload: Record<string, unknown>) => {
      emitted.push({ eventName, payload });
    }),
    emitted,
  };
}

describe('TransactionNotifier ↔ JayreadBus bridge', () => {
  let bus: ReturnType<typeof createMockBus>;
  let notifier: TransactionNotifier;

  beforeEach(() => {
    bus = createMockBus();
    notifier = new TransactionNotifier(bus);
  });

  it('事务 commit 后 bridge 转发事件到 bus', async () => {
    notifier.begin();
    notifier.trigger('add', 'paper', ['p1', 'p2']);
    notifier.trigger('modify', 'annotation', ['a1']);
    await notifier.commit();

    expect(bus.emit).toHaveBeenCalledTimes(2);

    // 确定性顺序：paper 先于 annotation
    expect(bus.emitted[0].eventName).toBe('paper.add');
    expect(bus.emitted[0].payload.ids).toEqual(['p1', 'p2']);

    expect(bus.emitted[1].eventName).toBe('annotation.modify');
    expect(bus.emitted[1].payload.ids).toEqual(['a1']);
  });

  it('非事务 trigger 也转发到 bus', async () => {
    await notifier.trigger('delete', 'paper', ['p1']);

    expect(bus.emit).toHaveBeenCalledTimes(1);
    expect(bus.emitted[0].eventName).toBe('paper.delete');
    expect(bus.emitted[0].payload.ids).toEqual(['p1']);
  });

  it('事务内去重后 bus 只收到合并后的事件', async () => {
    notifier.begin();
    notifier.trigger('modify', 'paper', ['p1']);
    notifier.trigger('modify', 'paper', ['p1']);
    notifier.trigger('modify', 'paper', ['p1']);
    await notifier.commit();

    // 三次 modify 合并为一次
    expect(bus.emit).toHaveBeenCalledTimes(1);
    expect(bus.emitted[0].eventName).toBe('paper.modify');
    expect(bus.emitted[0].payload.ids).toEqual(['p1']);
  });

  it('add + delete 互消后 bus 不收到任何事件', async () => {
    notifier.begin();
    notifier.trigger('add', 'paper', ['p1']);
    notifier.trigger('delete', 'paper', ['p1']);
    await notifier.commit();

    expect(bus.emit).toHaveBeenCalledTimes(0);
  });

  it('extraData 合并转发到 bus', async () => {
    notifier.begin();
    notifier.trigger('add', 'paper', ['p1'], { source: 'import' });
    await notifier.commit();

    expect(bus.emitted[0].payload.ids).toEqual(['p1']);
  });

  it('bridge observer 优先级最低，业务观察者先执行', async () => {
    const order: string[] = [];

    notifier.registerObserver({
      notify: vi.fn(async () => {
        order.push('business');
      }),
    }, { priority: 10 });

    await notifier.trigger('add', 'paper', ['p1']);

    // bridge (priority 999) 在 business (priority 10) 之后
    expect(order).toEqual(['business']);
    expect(bus.emit).toHaveBeenCalledTimes(1);
  });

  it('不注入 bus 时不转发', async () => {
    const noBusNotifier = new TransactionNotifier();

    // 应该不抛异常
    await noBusNotifier.trigger('add', 'paper', ['p1']);

    noBusNotifier.begin();
    noBusNotifier.trigger('modify', 'paper', ['p1']);
    await noBusNotifier.commit();
  });

  it('确定性排序反映在 bus 接收顺序中', async () => {
    notifier.begin();
    // 乱序触发
    notifier.trigger('delete', 'annotation', ['a1']);
    notifier.trigger('modify', 'paper', ['p1']);
    notifier.trigger('add', 'paper', ['p2']);
    notifier.trigger('add', 'annotation', ['a2']);
    await notifier.commit();

    const names = bus.emitted.map((e) => e.eventName);
    // paper 先于 annotation；add 先于 modify/delete
    expect(names).toEqual([
      'paper.add',
      'paper.modify',
      'annotation.add',
      'annotation.delete',
    ]);
  });
});
