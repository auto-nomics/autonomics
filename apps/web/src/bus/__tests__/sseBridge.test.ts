import { describe, it, expect, vi, beforeEach, type Mock } from 'vitest';
import { JayreadBus } from '../index';

// 先 mock 掉 JayreadBus.emit 来捕获转发事件
const emitted: Array<{ eventName: string; payload: Record<string, unknown> }> = [];
const originalEmit = JayreadBus.emit;

// 我们需要通过 import notifier 来测试——但由于 notifier 单例已经桥接到 JayreadBus，
// 可以通过监听 JayreadBus 事件来验证 sseBridge 调用是否正确传播
import {
  bridgeParseDone,
  bridgeParseError,
  bridgePaperAdded,
  bridgePaperDeleted,
  bridgeFolderAdded,
  bridgeFolderModified,
  bridgeFolderDeleted,
} from '../sseBridge';

describe('sseBridge — SSE → notifier 桥接', () => {
  let handler: Mock;

  beforeEach(() => {
    handler = vi.fn();
  });

  describe('bridgeParseDone', () => {
    it('触发 paper.modify 事件，包含 parseStatus: completed', async () => {
      const unsub = JayreadBus.on('paper.modify' as any, (payload: unknown) => {
        handler(payload);
      });

      await bridgeParseDone(42, 'mineru');

      expect(handler).toHaveBeenCalledTimes(1);
      const payload = handler.mock.calls[0][0] as Record<string, unknown>;
      expect(payload.ids).toEqual(['42']);
      expect(payload.parseStatus).toBe('completed');
      expect(payload.parseEngine).toBe('mineru');

      unsub();
    });
  });

  describe('bridgeParseError', () => {
    it('触发 paper.modify 事件，包含 parseStatus: error', async () => {
      const unsub = JayreadBus.on('paper.modify' as any, (payload: unknown) => {
        handler(payload);
      });

      await bridgeParseError(7, 'PDF corrupted');

      expect(handler).toHaveBeenCalledTimes(1);
      const payload = handler.mock.calls[0][0] as Record<string, unknown>;
      expect(payload.ids).toEqual(['7']);
      expect(payload.parseStatus).toBe('error');
      expect(payload.error).toBe('PDF corrupted');

      unsub();
    });
  });

  describe('bridgePaperAdded', () => {
    it('触发 paper.add 事件', async () => {
      const unsub = JayreadBus.on('paper.add' as any, (payload: unknown) => {
        handler(payload);
      });

      await bridgePaperAdded(100, { source: 'upload' });

      expect(handler).toHaveBeenCalledTimes(1);
      const payload = handler.mock.calls[0][0] as Record<string, unknown>;
      expect(payload.ids).toEqual(['100']);
      expect(payload.source).toBe('upload');

      unsub();
    });
  });

  describe('bridgePaperDeleted', () => {
    it('触发 paper.delete 事件', async () => {
      const unsub = JayreadBus.on('paper.delete' as any, (payload: unknown) => {
        handler(payload);
      });

      await bridgePaperDeleted(55);

      expect(handler).toHaveBeenCalledTimes(1);
      const payload = handler.mock.calls[0][0] as Record<string, unknown>;
      expect(payload.ids).toEqual(['55']);

      unsub();
    });
  });

  describe('bridgeFolderAdded', () => {
    it('触发 folder.add 事件', async () => {
      const unsub = JayreadBus.on('folder.add' as any, (payload: unknown) => {
        handler(payload);
      });

      await bridgeFolderAdded('col-1', { name: 'My Collection' });

      expect(handler).toHaveBeenCalledTimes(1);
      const payload = handler.mock.calls[0][0] as Record<string, unknown>;
      expect(payload.ids).toEqual(['col-1']);
      expect(payload.name).toBe('My Collection');

      unsub();
    });
  });

  describe('bridgeFolderModified', () => {
    it('触发 folder.modify 事件', async () => {
      const unsub = JayreadBus.on('folder.modify' as any, (payload: unknown) => {
        handler(payload);
      });

      await bridgeFolderModified('col-1');

      expect(handler).toHaveBeenCalledTimes(1);
      expect(handler.mock.calls[0][0].ids).toEqual(['col-1']);

      unsub();
    });
  });

  describe('bridgeFolderDeleted', () => {
    it('触发 folder.delete 事件', async () => {
      const unsub = JayreadBus.on('folder.delete' as any, (payload: unknown) => {
        handler(payload);
      });

      await bridgeFolderDeleted('col-1');

      expect(handler).toHaveBeenCalledTimes(1);
      expect(handler.mock.calls[0][0].ids).toEqual(['col-1']);

      unsub();
    });
  });
});
