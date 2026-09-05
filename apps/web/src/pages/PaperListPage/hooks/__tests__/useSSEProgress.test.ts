/**
 * useSSEProgress 单元测试（fetch-SSE 读取器版）
 *
 * 用 mock fetch 返回构造的 SSE 字节流，覆盖：
 * - progress → done 的回调序列（snake_case 载荷 → camelCase 回调）与成功 toast
 * - ping 心跳被忽略
 * - 帧跨 chunk 分割时的残帧缓冲（半帧留 buffer 等下一块再解析）
 * - error 业务事件：onError + 错误 toast，且关流后**不**进入网络重试
 * - 网络断开（流关闭无终态）的指数退避：1s → 2s → 4s，上限 3 次重试
 * - cleanup / 卸载：abort 全部连接、清重试定时器
 * - 重复 listen 同一论文：旧连接被 abort
 * - reconnectPendingPapers：只为 pending/processing/parsing 且无活跃连接的论文重连
 *
 * 鉴权：本地有 token 时 fetch 带 Authorization: Bearer（EventSource 做不到的点）
 */

import { describe, it, expect, vi, beforeAll, afterAll, beforeEach, afterEach } from 'vitest';
import { renderHook, act, waitFor } from '@testing-library/react';
import type { Paper } from '@/types';

// setup.ts 全局 mock 了整个 antd（只有 Tooltip/Tag 组件）——本文件本地覆写，
// 提供钩子实际用到的 App.useApp。vi.hoisted 让 message 实例可被断言
const mocks = vi.hoisted(() => ({
  message: { success: vi.fn(), error: vi.fn() },
}));
vi.mock('antd', () => ({
  App: { useApp: () => ({ message: mocks.message }) },
}));
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

import { useSSEProgress } from '../useSSEProgress';

/** 构造一个 SSE 字节流 Response：frames 逐块吐出，之后流关闭 */
function sseResponse(frames: string[]): Response {
  const encoder = new TextEncoder();
  let i = 0;
  return {
    ok: true,
    status: 200,
    body: {
      getReader: () => ({
        read: () =>
          Promise.resolve(
            i < frames.length
              ? { done: false, value: encoder.encode(frames[i++]) }
              : { done: true },
          ),
      }),
    },
  } as unknown as Response;
}

/** 永不 resolve 的流：连接停在 read 上不关闭——测 cleanup/unmount 的
 * abort 与「重复 listen 断旧连」时，保证注册表里的连接还活着 */
function hangingResponse(): Response {
  return {
    ok: true,
    status: 200,
    body: {
      getReader: () => ({
        read: () => new Promise<never>(() => {}),
      }),
    },
  } as unknown as Response;
}

/** 装一个 fetch mock，返回给定 SSE 流；记录每次调用的 (url, init) */
function sseFetch(framesFactory: () => string[]) {
  const f = vi.fn((_url: unknown, init?: RequestInit) => {
    // 捕获 signal 供断言 abort
    void init;
    return Promise.resolve(sseResponse(framesFactory()));
  });
  global.fetch = f as unknown as typeof fetch;
  return f;
}

function fetchCallsTo(urlPart: string, f: ReturnType<typeof sseFetch>) {
  return f.mock.calls.filter(([u]) => String(u).includes(urlPart));
}

beforeAll(() => {
  // happy-dom 环境的 localStorage 是 Node 的空 stub（无任何方法），手动装一个
  const store = new Map<string, string>();
  vi.stubGlobal('localStorage', {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, value: string) => void store.set(key, String(value)),
    removeItem: (key: string) => void store.delete(key),
    clear: () => store.clear(),
  });
});

afterAll(() => {
  vi.unstubAllGlobals();
});

beforeEach(() => {
  vi.clearAllMocks();
  try { localStorage.removeItem('autonomics_token'); } catch { /* 隐私模式等 */ }
});

afterEach(() => {
  vi.useRealTimers();
});

describe('useSSEProgress — 事件序列', () => {
  it('progress → done：camelCase 回调按序触发，ping 忽略，done 后关连接不重试', async () => {
    localStorage.setItem('autonomics_token', ' test-token '); // 带空格：断言 trim
    const f = sseFetch(() => [
      'event: ping\ndata: {}\n\n',
      'event: progress\ndata: {"percent":5,"stage":"上传中"}\n\n',
      'event: progress\ndata: {"percent":42,"stage":"解析中 3/10 页"}\n\n',
      'event: done\ndata: {"parse_engine":"mineru","markdown_length":12000}\n\n',
    ]);
    const onProgress = vi.fn();
    const onDone = vi.fn();
    const onError = vi.fn();

    const { result } = renderHook(() => useSSEProgress({ onProgress, onDone, onError }));
    act(() => {
      result.current.listenToParseStatus('p1');
    });

    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));

    // 鉴权头：bearer 层拦 /api/**，EventSource 设不了、fetch 读取器可以
    const [url, init] = f.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/v1/bib/articles/p1/parse/stream'); // paperId 直接透传
    expect((init.headers as Record<string, string>).Authorization).toBe('Bearer test-token');

    expect(onProgress).toHaveBeenCalledTimes(2); // ping 不产生回调
    expect(onProgress).toHaveBeenNthCalledWith(2, {
      paperId: 'p1',
      percent: 42,
      stage: '解析中 3/10 页',
    });
    expect(onDone).toHaveBeenCalledWith({
      paperId: 'p1',
      parseEngine: 'mineru',
      markdownLength: 12000,
    });
    expect(onError).not.toHaveBeenCalled();
    expect(mocks.message.success).toHaveBeenCalledWith('message.reparseDone');

    // 终态后连接关闭，无重试定时器（真实计时器下不再有第二次 fetch）
    expect(f).toHaveBeenCalledTimes(1);
  });

  it('帧跨 chunk 分割：残帧留在缓冲区，拼齐后才解析', async () => {
    sseFetch(() => [
      'event: progress\ndata: {"percent":7,"sta',
      'ge":"解析中"}\n\nevent: done\ndata: {"parse_engine":"mineru","markdown_length":3}\n\n',
    ]);
    const onProgress = vi.fn();
    const onDone = vi.fn();

    const { result } = renderHook(() => useSSEProgress({ onProgress, onDone }));
    act(() => {
      result.current.listenToParseStatus('p2');
    });

    await waitFor(() => expect(onDone).toHaveBeenCalled());
    expect(onProgress).toHaveBeenCalledTimes(1);
    expect(onProgress).toHaveBeenCalledWith({ paperId: 'p2', percent: 7, stage: '解析中' });
  });

  it('error 业务事件：onError + 错误 toast，关流后不进入网络重试', async () => {
    vi.useFakeTimers();
    const f = sseFetch(() => [
      'event: error\ndata: {"message":"MinerU parse failed: quota"}\n\n',
    ]);
    const onError = vi.fn();
    const onDone = vi.fn();

    const { result } = renderHook(() => useSSEProgress({ onError, onDone }));
    await act(async () => {
      result.current.listenToParseStatus('p3');
      // fake timers 下 waitFor 会挂起：直接冲微任务把读循环推到 error 帧
      await vi.advanceTimersByTimeAsync(0);
    });

    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledWith({ paperId: 'p3', message: 'MinerU parse failed: quota' });
    expect(mocks.message.error).toHaveBeenCalledWith('MinerU parse failed: quota');
    expect(onDone).not.toHaveBeenCalled();

    // 业务错误不重连：即使时间流逝也没有第二次 fetch
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(f).toHaveBeenCalledTimes(1);
  });
});

describe('useSSEProgress — 网络断开重试', () => {
  it('流关闭无终态：指数退避 1s → 2s → 4s 各重试一次，共 3 次后放弃', async () => {
    vi.useFakeTimers();
    const f = sseFetch(() => []); // 立即关闭的流：无终态事件

    const { result } = renderHook(() => useSSEProgress({}));
    await act(async () => {
      result.current.listenToParseStatus('p1');
    });
    expect(f).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(f).toHaveBeenCalledTimes(2);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    expect(f).toHaveBeenCalledTimes(3);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(4000);
    });
    expect(f).toHaveBeenCalledTimes(4);

    // 超过 3 次重试后放弃，等待用户手动重解析
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(f).toHaveBeenCalledTimes(4);
  });

  it('重试成功收到 done 后停止（不再继续剩余重试）', async () => {
    vi.useFakeTimers();
    let calls = 0;
    const f = vi.fn(() => {
      calls += 1;
      // 第一次断流触发重试；第二次送达完整进度 + 终态
      return Promise.resolve(sseResponse(calls === 1 ? [] : [
        'event: done\ndata: {"parse_engine":"mineru","markdown_length":9}\n\n',
      ]));
    });
    global.fetch = f as unknown as typeof fetch;
    const onDone = vi.fn();

    const { result } = renderHook(() => useSSEProgress({ onDone }));
    await act(async () => {
      result.current.listenToParseStatus('p1');
    });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000); // 1s 后第一次重试
    });
    expect(f).toHaveBeenCalledTimes(2);
    // 冲微任务：重试连接读到 done 帧并终态收尾（waitFor 在 fake timers 下会挂起）
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(onDone).toHaveBeenCalled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(f).toHaveBeenCalledTimes(2); // done 终态：不再重试
  });
});

describe('useSSEProgress — 连接管理', () => {
  it('重复 listen 同一论文：旧连接被 abort（防双连接）', () => {
    // 挂起流：第一条连接停在 read 上不关闭，注册表里保持在场
    const f = vi.fn(() => Promise.resolve(hangingResponse()));
    global.fetch = f as unknown as typeof fetch;
    const { result } = renderHook(() => useSSEProgress({}));

    act(() => {
      result.current.listenToParseStatus('p1');
    });
    const first = (f.mock.calls[0] as [string, RequestInit])[1].signal as AbortSignal;
    expect(first.aborted).toBe(false);

    act(() => {
      result.current.listenToParseStatus('p1'); // 快速重触发（如重解析）
    });
    const second = (f.mock.calls[1] as [string, RequestInit])[1].signal as AbortSignal;
    expect(first.aborted).toBe(true);
    expect(second.aborted).toBe(false);
  });

  it('cleanup：abort 全部连接并清掉挂起的重试定时器', async () => {
    vi.useFakeTimers();
    let calls = 0;
    const f = vi.fn((_url: unknown) => {
      calls += 1;
      // p1 挂起保活（测 abort）；p2 断流（读循环走到 onDrop 挂 1s 重试定时器）
      return Promise.resolve(calls === 1 ? hangingResponse() : sseResponse([]));
    });
    global.fetch = f as unknown as typeof fetch;
    const { result } = renderHook(() => useSSEProgress({}));

    await act(async () => {
      result.current.listenToParseStatus('p1');
      result.current.listenToParseStatus('p2');
      await vi.advanceTimersByTimeAsync(0); // 冲微任务：p2 的读循环跑到 onDrop
    });
    const p1Signal = (f.mock.calls[0] as [string, RequestInit])[1].signal as AbortSignal;
    expect(f).toHaveBeenCalledTimes(2); // 尚未重试

    act(() => {
      result.current.cleanup();
    });
    expect(p1Signal.aborted).toBe(true);

    // p2 的重试定时器已被清理：时间流逝也不重连
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(f).toHaveBeenCalledTimes(2);
  });

  it('组件卸载时自动清理（hook 内部的 useEffect 兜底）', () => {
    const f = vi.fn(() => Promise.resolve(hangingResponse()));
    global.fetch = f as unknown as typeof fetch;
    const { result, unmount } = renderHook(() => useSSEProgress({}));

    act(() => {
      result.current.listenToParseStatus('p1');
    });
    const signal = (f.mock.calls[0] as [string, RequestInit])[1].signal as AbortSignal;

    unmount();
    expect(signal.aborted).toBe(true);
  });
});

describe('useSSEProgress — reconnectPendingPapers', () => {
  it('为 pending/processing/parsing 且无活跃连接的论文重连；done/failed 不连；只执行一次', async () => {
    const f = sseFetch(() => []);
    const { result } = renderHook(() => useSSEProgress({}));

    const papers = [
      { id: 'p1', parse_status: 'pending' },
      { id: 'p2', parse_status: 'processing' },
      { id: 'p3', parse_status: 'parsing' }, // 前端 SSE 乐观态
      { id: 'p4', parse_status: 'done' },
      { id: 'p5', parse_status: 'failed' },
    ] as unknown as Paper[];
    const ref = { current: false };

    await act(async () => {
      result.current.reconnectPendingPapers(papers, false, ref);
    });
    expect(fetchCallsTo('/parse/stream', f)).toHaveLength(3); // p1 p2 p3
    expect(ref.current).toBe(true);

    // 标记位置位后不再触发（避免 done 过渡期误重连）
    await act(async () => {
      result.current.reconnectPendingPapers(papers, false, ref);
    });
    expect(fetchCallsTo('/parse/stream', f)).toHaveLength(3);

    // loading 时跳过（等 loadPapers 完成）
    const ref2 = { current: false };
    await act(async () => {
      result.current.reconnectPendingPapers(papers, true, ref2);
    });
    expect(ref2.current).toBe(false);
    expect(fetchCallsTo('/parse/stream', f)).toHaveLength(3);
  });

  it('已有活跃连接的论文不重复开流', async () => {
    const f = sseFetch(() => [
      'event: progress\ndata: {"percent":10,"stage":"解析中"}\n\n',
    ]);
    const { result } = renderHook(() => useSSEProgress({}));

    act(() => {
      result.current.listenToParseStatus('p1');
    });

    const papers = [{ id: 'p1', parse_status: 'pending' }] as unknown as Paper[];
    await act(async () => {
      result.current.reconnectPendingPapers(papers, false, { current: false });
    });
    expect(fetchCallsTo('/parse/stream', f)).toHaveLength(1); // 未开第二条
  });
});
