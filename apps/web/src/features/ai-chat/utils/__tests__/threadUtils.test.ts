/**
 * threadUtils 工具函数测试文件
 *
 * 测试内联线程功能，包括：
 * - cyrb53 哈希函数
 * - 线程创建和管理
 * - 线程数据的不可变更新
 * - 线程结构规范化
 */

import { describe, it, expect } from 'vitest';
import {
  cyrb53,
  computeFingerprint,
  createThread,
  addMessageToThread,
  updateLastThreadMessageContent,
  updateThreadInMessages,
  markThreadsOrphaned,
  ensureThreadStructure,
  buildThreadSystemPrompt,
  MAX_THREAD_MESSAGES,
  MAX_THREADS_PER_MESSAGE,
  ANCHOR_TEXT_MAX_TOKENS,
  THREAD_TOKEN_BUDGET,
} from '../threadUtils';

// ============================================================
// cyrb53 哈希函数测试组
// ============================================================

describe('cyrb53', () => {
  // 测试 01: 基本功能 - 返回数字
  it('01: returns a number for string input', () => {
    const result = cyrb53('hello');
    expect(typeof result).toBe('number');
  });

  // 测试 02: 确定性 - 相同输入产生相同输出
  it('02: same input produces same output (deterministic)', () => {
    const input = 'test string';
    const result1 = cyrb53(input);
    const result2 = cyrb53(input);
    expect(result1).toBe(result2);
  });

  // 测试 03: 不同输入产生不同输出
  it('03: different inputs produce different outputs', () => {
    const result1 = cyrb53('hello');
    const result2 = cyrb53('world');
    expect(result1).not.toBe(result2);
  });

  // 测试 04: 种子参数产生不同结果
  it('04: different seeds produce different results for same input', () => {
    const input = 'hello';
    const result1 = cyrb53(input, 1);
    const result2 = cyrb53(input, 2);
    expect(result1).not.toBe(result2);
  });

  // 测试 05: 相同种子产生相同结果
  it('05: same seed produces same result', () => {
    const input = 'test';
    const seed = 42;
    const result1 = cyrb53(input, seed);
    const result2 = cyrb53(input, seed);
    expect(result1).toBe(result2);
  });

  // 测试 06: 空字符串产生有效哈希
  it('06: empty string produces valid hash', () => {
    const result = cyrb53('');
    expect(typeof result).toBe('number');
    expect(result).not.toBeNaN();
  });

  // 测试 07: Unicode 字符正确处理
  it('07: handles unicode characters correctly', () => {
    const result1 = cyrb53('你好世界');
    const result2 = cyrb53('こんにちは');
    expect(typeof result1).toBe('number');
    expect(typeof result2).toBe('number');
    expect(result1).not.toBe(result2);
  });

  // 测试 08: 返回安全整数（在 2^53 - 1 内）
  it('08: returns safe integer (within Number.MAX_SAFE_INTEGER)', () => {
    const result = cyrb53('any string');
    expect(Number.isSafeInteger(result)).toBe(true);
  });

  // 测试 09: 大小写敏感
  it('09: is case sensitive', () => {
    const result1 = cyrb53('Hello');
    const result2 = cyrb53('hello');
    expect(result1).not.toBe(result2);
  });

  // 测试 10: 无冲突测试（10,000 组合）
  it('10: no collisions for 10,000 different inputs', () => {
    const hashes = new Set();
    const inputs = [];

    // 生成 10,000 个不同的输入字符串
    for (let i = 0; i < 10000; i++) {
      inputs.push(`string-${i}-suffix`);
    }

    // 检查冲突
    for (const input of inputs) {
      const hash = cyrb53(input);
      if (hashes.has(hash)) {
        // 检测到冲突！
        expect.fail(`Collision detected for hash ${hash}`);
      }
      hashes.add(hash);
    }

    // 所有哈希应该唯一
    expect(hashes.size).toBe(10000);
  });
});

// ============================================================
// computeFingerprint 测试组
// ============================================================

describe('computeFingerprint', () => {
  // 测试 11: 返回字符串
  it('11: returns string fingerprint', () => {
    const result = computeFingerprint('hello world', 0);
    expect(typeof result).toBe('string');
  });

  // 测试 12: 相同内容和偏移量产生相同指纹
  it('12: same content and offset produce same fingerprint', () => {
    const content = 'the method uses attention';
    const offset = 10;
    const result1 = computeFingerprint(content, offset);
    const result2 = computeFingerprint(content, offset);
    expect(result1).toBe(result2);
  });

  // 测试 13: 不同偏移量产生不同指纹
  it('13: different offsets produce different fingerprints for same text', () => {
    const content = 'the method uses attention';
    const result1 = computeFingerprint(content, 0);
    const result2 = computeFingerprint(content, 10);
    const result3 = computeFingerprint(content, 100);
    expect(result1).not.toBe(result2);
    expect(result1).not.toBe(result3);
    expect(result2).not.toBe(result3);
  });

  // 测试 14: 不同内容产生不同指纹
  it('14: different content produces different fingerprints', () => {
    const result1 = computeFingerprint('hello', 0);
    const result2 = computeFingerprint('world', 0);
    expect(result1).not.toBe(result2);
  });

  // 测试 15: 偏移量 0 正确工作
  it('15: offset 0 produces valid fingerprint', () => {
    const result = computeFingerprint('test', 0);
    expect(typeof result).toBe('string');
    expect(result.length).toBeGreaterThan(0);
  });

  // 测试 16: 大偏移量正确工作
  it('16: large offset produces valid fingerprint', () => {
    const result = computeFingerprint('test', 999999);
    expect(typeof result).toBe('string');
    expect(result.length).toBeGreaterThan(0);
  });

  // 测试 17: 带偏移量的空内容
  it('17: empty content with offset produces valid fingerprint', () => {
    const result = computeFingerprint('', 100);
    expect(typeof result).toBe('string');
  });
});

// ============================================================
// createThread 测试组
// ============================================================

describe('createThread', () => {
  // 测试 18: 创建正确结构的线程
  it('18: creates thread with correct structure', () => {
    const thread = createThread('fp123', 'selected text');
    expect(thread).toHaveProperty('id');
    expect(thread).toHaveProperty('anchorFingerprint', 'fp123');
    expect(thread).toHaveProperty('anchorText', 'selected text');
    expect(thread).toHaveProperty('orphaned', false);
    expect(thread).toHaveProperty('messages');
    expect(Array.isArray(thread.messages)).toBe(true);
    expect(thread.messages.length).toBe(0);
  });

  // 测试 19: 为不同线程生成唯一 ID
  it('19: generates unique IDs for different threads', () => {
    const thread1 = createThread('fp1', 'text1');
    const thread2 = createThread('fp1', 'text1');
    expect(thread1.id).not.toBe(thread2.id);
  });

  // 测试 20: 线程 ID 是非空字符串
  it('20: thread ID is a non-empty string', () => {
    const thread = createThread('fp123', 'text');
    expect(typeof thread.id).toBe('string');
    expect(thread.id.length).toBeGreaterThan(0);
  });

  // 测试 21: 允许同一指纹对应多个线程
  it('21: allows same fingerprint for multiple threads (same paragraph, multiple questions)', () => {
    const thread1 = createThread('fp-same', 'question 1');
    const thread2 = createThread('fp-same', 'question 2');
    expect(thread1.anchorFingerprint).toBe(thread2.anchorFingerprint);
    expect(thread1.id).not.toBe(thread2.id);
  });
});

// ============================================================
// addMessageToThread 测试组
// ============================================================

describe('addMessageToThread', () => {
  // 测试 22: 向空线程添加消息
  it('22: adds message to empty thread', () => {
    const thread = createThread('fp', 'text');
    const updated = addMessageToThread(thread, { role: 'user', content: 'Why?' });
    expect(updated.messages.length).toBe(1);
    expect(updated.messages[0].role).toBe('user');
    expect(updated.messages[0].content).toBe('Why?');
  });

  // 测试 23: 追加消息到现有消息
  it('23: appends message to existing messages', () => {
    const thread = createThread('fp', 'text');
    const withFirst = addMessageToThread(thread, { role: 'user', content: 'First' });
    const withSecond = addMessageToThread(withFirst, { role: 'assistant', content: 'Answer' });
    expect(withSecond.messages.length).toBe(2);
    expect(withSecond.messages[0].content).toBe('First');
    expect(withSecond.messages[1].content).toBe('Answer');
  });

  // 测试 24: 为无 ID 的消息生成 ID
  it('24: generates ID for message without ID', () => {
    const thread = createThread('fp', 'text');
    const updated = addMessageToThread(thread, { role: 'user', content: 'Test' });
    expect(typeof updated.messages[0].id).toBe('string');
    expect(updated.messages[0].id.length).toBeGreaterThan(0);
  });

  // 测试 25: 保留现有消息 ID
  it('25: preserves existing message ID', () => {
    const thread = createThread('fp', 'text');
    const existingId = 'msg-123';
    const updated = addMessageToThread(thread, { id: existingId, role: 'user', content: 'Test' });
    expect(updated.messages[0].id).toBe(existingId);
  });

  // 测试 26: 返回新对象（不可变）
  it('26: returns new object (immutable update)', () => {
    const thread = createThread('fp', 'text');
    const updated = addMessageToThread(thread, { role: 'user', content: 'Test' });
    expect(thread).not.toBe(updated);
    expect(thread.messages).not.toBe(updated.messages);
  });
});

// ============================================================
// updateLastThreadMessageContent 测试组
// ============================================================

describe('updateLastThreadMessageContent', () => {
  // 测试 27: 更新最后一条消息内容
  it('27: updates last message content', () => {
    const thread = createThread('fp', 'text');
    const withMsg = addMessageToThread(thread, { role: 'assistant', content: 'Hello' });
    const updated = updateLastThreadMessageContent(withMsg, 'Hello world');
    expect(updated.messages[0].content).toBe('Hello world');
  });

  // 测试 28: 保留最后消息的 ID 和角色
  it('28: preserves last message ID and role', () => {
    const thread = createThread('fp', 'text');
    const withMsg = addMessageToThread(thread, { id: 'msg-1', role: 'assistant', content: 'Hi' });
    const updated = updateLastThreadMessageContent(withMsg, 'Hi there');
    expect(updated.messages[0].id).toBe('msg-1');
    expect(updated.messages[0].role).toBe('assistant');
  });

  // 测试 29: 处理空线程（无消息）
  it('29: handles empty thread gracefully', () => {
    const thread = createThread('fp', 'text');
    const updated = updateLastThreadMessageContent(thread, 'content');
    expect(updated).toBe(thread); // 应返回相同引用
  });

  // 测试 30: 多条消息时只更新最后一条
  it('30: only updates last message when multiple exist', () => {
    const thread = createThread('fp', 'text');
    const t1 = addMessageToThread(thread, { role: 'user', content: 'Q1' });
    const t2 = addMessageToThread(t1, { role: 'assistant', content: 'A1' });
    const t3 = addMessageToThread(t2, { role: 'user', content: 'Q2' });
    const updated = updateLastThreadMessageContent(t3, 'Q2 updated');
    expect(updated.messages[0].content).toBe('Q1'); // 未改变
    expect(updated.messages[1].content).toBe('A1'); // 未改变
    expect(updated.messages[2].content).toBe('Q2 updated'); // 已更新
  });
});

// ============================================================
// updateThreadInMessages 测试组
// ============================================================

describe('updateThreadInMessages', () => {
  // 测试 31: 更新消息中的指定线程
  it('31: updates specified thread in message', () => {
    const messages = [
      { id: 'm1', role: 'assistant', content: 'Answer', threads: [
        createThread('fp1', 'text1'),
        createThread('fp2', 'text2'),
      ]},
    ];
    const updated = updateThreadInMessages(messages, 0, messages[0].threads[1].id, (t) => ({
      ...t,
      orphaned: true,
    }));
    expect(updated[0].threads[0].orphaned).toBe(false); // 未改变
    expect(updated[0].threads[1].orphaned).toBe(true); // 已更新
  });

  // 测试 32: 使用定向展开（保留未变消息引用）
  it('32: uses targeted spread - preserves unchanged message references', () => {
    const messages = [
      { id: 'm1', role: 'user', content: 'Q1', threads: [] },
      { id: 'm2', role: 'assistant', content: 'A1', threads: [createThread('fp', 'text')] },
      { id: 'm3', role: 'user', content: 'Q2', threads: [] },
    ];
    const updated = updateThreadInMessages(messages, 1, messages[1].threads[0].id, (t) => ({
      ...t,
      orphaned: true,
    }));

    // m1 和 m3 应该有相同引用（无复制）
    expect(updated[0]).toBe(messages[0]);
    expect(updated[2]).toBe(messages[2]);
    // m2 应该是新引用
    expect(updated[1]).not.toBe(messages[1]);
  });

  // 测试 33: 保留未变线程引用
  it('33: preserves unchanged thread references', () => {
    const thread1 = createThread('fp1', 'text1');
    const thread2 = createThread('fp2', 'text2');
    const messages = [
      { id: 'm1', role: 'assistant', content: 'A', threads: [thread1, thread2] },
    ];
    const updated = updateThreadInMessages(messages, 0, thread2.id, (t) => ({
      ...t,
      orphaned: true,
    }));

    // thread1 应该有相同引用
    expect(updated[0].threads[0]).toBe(thread1);
    // thread2 应该是新引用
    expect(updated[0].threads[1]).not.toBe(thread2);
  });

  // 测试 34: 处理返回相同引用的更新器
  it('34: handles updater that modifies and returns new object', () => {
    const messages = [
      { id: 'm1', role: 'assistant', content: 'A', threads: [createThread('fp', 'text')] },
    ];
    const threadId = messages[0].threads[0].id;
    const updated = updateThreadInMessages(messages, 0, threadId, (t) => t); // 恒等函数

    // 由于更新器中的展开操作，仍应创建新消息和线程数组
    expect(updated).not.toBe(messages);
    expect(updated[0]).not.toBe(messages[0]);
    expect(updated[0].threads).not.toBe(messages[0].threads);
  });
});

// ============================================================
// markThreadsOrphaned 测试组
// ============================================================

describe('markThreadsOrphaned', () => {
  // 测试 35: 将所有线程标记为孤立
  it('35: marks all threads as orphaned', () => {
    const messages = [
      { id: 'm1', role: 'assistant', content: 'A', threads: [
        createThread('fp1', 'text1'),
        createThread('fp2', 'text2'),
      ]},
    ];
    const updated = markThreadsOrphaned(messages, 0);
    expect(updated[0].threads[0].orphaned).toBe(true);
    expect(updated[0].threads[1].orphaned).toBe(true);
  });

  // 测试 36: 标记孤立时保留其他线程属性
  it('36: preserves other thread properties when marking orphaned', () => {
    const thread = createThread('fp', 'original text');
    const messages = [
      { id: 'm1', role: 'assistant', content: 'A', threads: [thread] },
    ];
    const updated = markThreadsOrphaned(messages, 0);
    expect(updated[0].threads[0].id).toBe(thread.id);
    expect(updated[0].threads[0].anchorFingerprint).toBe(thread.anchorFingerprint);
    expect(updated[0].threads[0].anchorText).toBe(thread.anchorText);
    expect(updated[0].threads[0].messages).toEqual(thread.messages);
  });

  // 测试 37: 只影响指定消息
  it('37: only affects specified message', () => {
    const thread1 = createThread('fp1', 'text1');
    const thread2 = createThread('fp2', 'text2');
    const messages = [
      { id: 'm1', role: 'assistant', content: 'A1', threads: [thread1] },
      { id: 'm2', role: 'assistant', content: 'A2', threads: [thread2] },
    ];
    const updated = markThreadsOrphaned(messages, 0);
    expect(updated[0].threads[0].orphaned).toBe(true);
    expect(updated[1].threads[0].orphaned).toBe(false);
  });

  // 测试 38: 处理无线程的消息
  it('38: handles message with no threads', () => {
    const messages = [
      { id: 'm1', role: 'assistant', content: 'A', threads: [] },
    ];
    const updated = markThreadsOrphaned(messages, 0);
    expect(updated[0].threads).toEqual([]);
  });
});

// ============================================================
// ensureThreadStructure 测试组
// ============================================================

describe('ensureThreadStructure', () => {
  // 测试 39: 将 null 线程规范化为空数组
  it('39: normalizes null threads to empty array', () => {
    const messages = [
      { id: 'm1', role: 'assistant', content: 'A', threads: null },
    ];
    const normalized = ensureThreadStructure(messages);
    expect(normalized[0].threads).toEqual([]);
  });

  // 测试 40: 将 undefined 线程规范化为空数组
  it('40: normalizes undefined threads to empty array', () => {
    const messages = [
      { id: 'm1', role: 'assistant', content: 'A' }, // 无 threads 字段
    ];
    const normalized = ensureThreadStructure(messages);
    expect(normalized[0].threads).toEqual([]);
  });

  // 测试 41: 保留现有 threads 数组
  it('41: preserves existing threads array', () => {
    const thread = createThread('fp', 'text');
    const messages = [
      { id: 'm1', role: 'assistant', content: 'A', threads: [thread] },
    ];
    const normalized = ensureThreadStructure(messages);
    expect(normalized[0].threads).toBe(messages[0].threads); // 相同引用
  });

  // 测试 42: 保留空数组
  it('42: preserves empty array', () => {
    const messages = [
      { id: 'm1', role: 'assistant', content: 'A', threads: [] },
    ];
    const normalized = ensureThreadStructure(messages);
    expect(normalized[0].threads).toEqual([]);
  });

  // 测试 43: 处理带混合线程状态的多个消息
  it('43: handles multiple messages with mixed thread states', () => {
    const thread = createThread('fp', 'text');
    const messages = [
      { id: 'm1', role: 'assistant', content: 'A1', threads: null },
      { id: 'm2', role: 'assistant', content: 'A2', threads: [thread] },
      { id: 'm3', role: 'assistant', content: 'A3' }, // 无 threads
    ];
    const normalized = ensureThreadStructure(messages);
    expect(normalized[0].threads).toEqual([]);
    expect(normalized[1].threads).toBe(messages[1].threads);
    expect(normalized[2].threads).toEqual([]);
  });
});

// ============================================================
// buildThreadSystemPrompt 测试组
// ============================================================

describe('buildThreadSystemPrompt', () => {
  // 测试 44: 在 prompt 中包含锚文本
  it('44: includes anchor text in prompt', () => {
    const threadContext = { anchorText: 'the method uses attention' };
    const prompt = buildThreadSystemPrompt(threadContext);
    expect(prompt).toContain('the method uses attention');
  });

  // 测试 45: 提供时包含助手回复
  it('45: includes assistant reply when provided', () => {
    const threadContext = {
      anchorText: 'method',
      assistantReply: 'This is the AI response.',
    };
    const prompt = buildThreadSystemPrompt(threadContext);
    expect(prompt).toContain('This is the AI response');
  });

  // 测试 46: 截断过长锚文本
  it('46: truncates long anchor text and adds [已截断] marker', () => {
    // ANCHOR_TEXT_MAX_TOKENS = 1000, maxChars = 4000
    const longText = 'a'.repeat(5000);
    const threadContext = { anchorText: longText };
    const prompt = buildThreadSystemPrompt(threadContext);
    expect(prompt).toContain('[已截断]');
  });

  // 测试 47: 包含简洁回答要求
  it('47: includes concise answer requirement in prompt', () => {
    const threadContext = { anchorText: 'text' };
    const prompt = buildThreadSystemPrompt(threadContext);
    // v2 (2026-06-28): 「简洁回答」改写为「简洁直接」+ 边界指引
    expect(prompt).toContain('简洁直接');
  });

  // 测试 48: 优雅处理 null 锚文本
  it('48: handles null anchor text gracefully', () => {
    const threadContext = { anchorText: null } as any;
    const prompt = buildThreadSystemPrompt(threadContext);
    expect(typeof prompt).toBe('string');
  });

  // 测试 49: 处理空助手回复
  it('49: handles empty assistant reply', () => {
    const threadContext = { anchorText: 'text', assistantReply: '' };
    const prompt = buildThreadSystemPrompt(threadContext);
    expect(typeof prompt).toBe('string');
  });
});

// ============================================================
// 常量测试组
// ============================================================

describe('Constants', () => {
  // 测试 50: MAX_THREAD_MESSAGES 常量
  it('50: MAX_THREAD_MESSAGES is defined correctly', () => {
    expect(MAX_THREAD_MESSAGES).toBe(30);
  });

  // 测试 51: MAX_THREADS_PER_MESSAGE 常量
  it('51: MAX_THREADS_PER_MESSAGE is defined correctly', () => {
    expect(MAX_THREADS_PER_MESSAGE).toBe(10);
  });

  // 测试 52: ANCHOR_TEXT_MAX_TOKENS 常量
  it('52: ANCHOR_TEXT_MAX_TOKENS is defined correctly', () => {
    expect(ANCHOR_TEXT_MAX_TOKENS).toBe(1000);
  });

  // 测试 53: THREAD_TOKEN_BUDGET 常量
  it('53: THREAD_TOKEN_BUDGET is defined correctly', () => {
    expect(THREAD_TOKEN_BUDGET).toBe(3000);
  });
});
