/**
 * chatMessageUtils 工具函数测试文件
 *
 * 测试多模态消息内容转换工具函数
 * 处理 OpenAI 和 Anthropic API 格式之间的转换
 */

import { describe, it, expect } from 'vitest';
import {
  convertToAnthropicContent,
  buildUserMessageContent,
  stripAttachmentsForPersistence,
  extractTextFromContent,
  extractImageUrlsFromContent,
  buildAnthropicSystemPrompt,
  generateMessageId,
  ensureMessageIds,
} from '../chatMessageUtils';

// ============================================================
// convertToAnthropicContent 测试组
// ============================================================

describe('convertToAnthropicContent', () => {
  // 测试 01: 字符串输入原样返回
  it('01: string input returns string unchanged', () => {
    const input = 'Hello, world!';
    const result = convertToAnthropicContent(input);
    expect(result).toBe('Hello, world!');
  });

  // 测试 02: 非数组、非字符串原样返回
  it('02: non-array, non-string returns as-is', () => {
    const obj = { foo: 'bar' };
    const result = convertToAnthropicContent(obj as any);
    expect(result).toBe(obj);
  });

  // 测试 03: 纯文本块数组合并为单个字符串（全文本优化）
  it('03: array of text parts gets merged to single string', () => {
    const input = [
      { type: 'text', text: 'First part' },
      { type: 'text', text: 'Second part' },
    ];
    const result = convertToAnthropicContent(input);
    // 当所有块都是文本时，合并为字符串进行优化
    expect(result).toBe('First partSecond part');
    expect(typeof result).toBe('string');
  });

  // 测试 04: 空文本块被过滤掉
  it('04: empty text parts are filtered out and merged to string', () => {
    const input = [
      { type: 'text', text: 'Valid text' },
      { type: 'text', text: '' },
      { type: 'text', text: null },
      { type: 'text', text: undefined },
      { type: 'text', text: 'Another valid' },
    ];
    const result = convertToAnthropicContent(input);
    // 空块被过滤后，剩余文本合并为字符串
    expect(result).toBe('Valid textAnother valid');
  });

  // 测试 05: 带 Data URL 的 image_url 块转换为 Anthropic 格式
  it('05: image_url with Data URL converts to Anthropic format', () => {
    const input = [
      {
        type: 'image_url',
        image_url: { url: 'data:image/png;base64,abc123def456' },
      },
    ];
    const result = convertToAnthropicContent(input);
    expect(result).toEqual([
      {
        type: 'image',
        source: {
          type: 'base64',
          media_type: 'image/png',
          data: 'abc123def456',
        },
      },
    ]);
  });

  // 测试 06: 字符串格式的 image_url
  it('06: image_url with string format (not object)', () => {
    const input = [
      {
        type: 'image_url',
        image_url: 'data:image/jpeg;base64,xyz789',
      },
    ];
    const result = convertToAnthropicContent(input);
    expect(result).toEqual([
      {
        type: 'image',
        source: {
          type: 'base64',
          media_type: 'image/jpeg',
          data: 'xyz789',
        },
      },
    ]);
  });

  // 测试 07: 无 URL 的 image_url 被跳过
  it('07: image_url with no URL is skipped, text merged to string', () => {
    const input = [
      { type: 'text', text: 'Some text' },
      { type: 'image_url', image_url: null },
      { type: 'image_url', image_url: { url: '' } },
      { type: 'image_url', image_url: '' },
    ];
    const result = convertToAnthropicContent(input);
    // 过滤后只剩下文本，合并为字符串
    expect(result).toBe('Some text');
  });

  // 测试 08: 已是 Anthropic 格式的直接透传
  it('08: already Anthropic format image block is passed through', () => {
    const anthropicImage = {
      type: 'image',
      source: { type: 'base64', media_type: 'image/png', data: 'abc123' },
    };
    const input = [{ type: 'text', text: 'Text' }, anthropicImage];
    const result = convertToAnthropicContent(input);
    expect(result).toEqual([{ type: 'text', text: 'Text' }, anthropicImage]);
  });

  // 测试 09: 未知类型块被跳过
  it('09: unknown part type is skipped, text merged to string', () => {
    const input = [
      { type: 'text', text: 'Valid text' },
      { type: 'unknown_type', data: 'something' },
      { type: 'audio_url', audio_url: { url: 'data:audio/wav;base64,abc' } },
    ];
    const result = convertToAnthropicContent(input);
    // 过滤未知类型后只保留文本
    expect(result).toBe('Valid text');
  });

  // 测试 10: 纯文本块合并为单个字符串
  it('10: all-text blocks are merged to single string (not array)', () => {
    const input = [
      { type: 'text', text: 'First' },
      { type: 'text', text: ' ' },
      { type: 'text', text: 'Second' },
    ];
    const result = convertToAnthropicContent(input);
    expect(result).toBe('First Second');
    expect(typeof result).toBe('string');
  });

  // 测试 11: Data URL 中的空白字符被移除
  it('11: Data URL with spaces and tabs has whitespace removed', () => {
    const input = [
      {
        type: 'image_url',
        image_url: { url: 'data:image/png;base64,abc 123\tdef 456' },
      },
    ];
    const result = convertToAnthropicContent(input);
    // 结果是包含图片块的数组（非全文本，不合并）
    expect(Array.isArray(result)).toBe(true);
    expect(result[0]).toEqual({
      type: 'image',
      source: {
        type: 'base64',
        media_type: 'image/png',
        data: 'abc123def456',
      },
    });
  });

  // 测试 12: 无效的 Data URL 格式被跳过
  it('12: invalid Data URL format is skipped', () => {
    const input = [
      { type: 'text', text: 'Text' },
      { type: 'image_url', image_url: { url: 'not-a-data-url' } },
      { type: 'image_url', image_url: { url: 'http://example.com/image.png' } },
    ];
    const result = convertToAnthropicContent(input);
    // 无效的图片 URL 被跳过，文本合并为字符串
    expect(result).toBe('Text');
  });
});

// ============================================================
// buildUserMessageContent 测试组
// ============================================================

describe('buildUserMessageContent', () => {
  // 测试 11: 无附件返回文本字符串
  it('11: no attachments returns text string', () => {
    const result = buildUserMessageContent('Hello world', null as any);
    expect(result).toBe('Hello world');
    expect(typeof result).toBe('string');
  });

  // 测试 12: null/空附件返回文本字符串
  it('12: null/empty attachments returns text string', () => {
    const result1 = buildUserMessageContent('Hello', null as any);
    const result2 = buildUserMessageContent('Hello', undefined);
    const result3 = buildUserMessageContent('Hello', []);
    const result4 = buildUserMessageContent('Hello', []);

    expect(result1).toBe('Hello');
    expect(result2).toBe('Hello');
    expect(result3).toBe('Hello');
    expect(result4).toBe('Hello');
  });

  // 测试 13: 图片附件返回带 image_url 的数组
  it('13: image attachment returns parts array with image_url', () => {
    const attachments = [
      { category: 'image', base64: 'data:image/png;base64,abc123' },
    ];
    const result = buildUserMessageContent('Check this image', attachments);
    expect(Array.isArray(result)).toBe(true);
    expect(result).toEqual([
      { type: 'text', text: 'Check this image' },
      {
        type: 'image_url',
        image_url: { url: 'data:image/png;base64,abc123' },
      },
    ]);
  });

  // 测试 14: 文本文件附件将内容注入文本并用代码块包裹
  it('14: text file attachment injects content with code fence', () => {
    const attachments = [
      {
        category: 'text',
        name: 'example.js',
        textContent: 'console.log("hello");',
        language: 'javascript',
      },
    ];
    const result = buildUserMessageContent('Look at this code', attachments);
    expect(result).toBe(
      'Look at this code\n\n--- 附件: example.js ---\n```javascript\nconsole.log("hello");\n```\n---'
    );
  });

  // 测试 15: 带 markdown 的二进制附件注入到文本
  it('15: binary attachment with markdown injects into text', () => {
    const attachments = [
      {
        category: 'binary',
        name: 'report.pdf',
        status: 'ready',
        markdown: '# Report Content\n\nThis is the converted PDF.',
      },
    ];
    const result = buildUserMessageContent('Review the report', attachments);
    expect(result).toBe(
      'Review the report\n\n--- 附件: report.pdf (已转换) ---\n# Report Content\n\nThis is the converted PDF.\n---'
    );
  });

  // 测试 16: 混合附件（图片 + 文本文件）返回数组
  it('16: mixed attachments returns parts array with both', () => {
    const attachments = [
      { category: 'image', base64: 'data:image/png;base64,abc123' },
      {
        category: 'text',
        name: 'code.js',
        textContent: 'const x = 1;',
        language: 'javascript',
      },
    ];
    const result = buildUserMessageContent('See this', attachments);
    expect(Array.isArray(result)).toBe(true);
    expect(result[0]).toEqual({
      type: 'text',
      text: 'See this\n\n--- 附件: code.js ---\n```javascript\nconst x = 1;\n```\n---',
    });
    expect(result[1]).toEqual({
      type: 'image_url',
      image_url: { url: 'data:image/png;base64,abc123' },
    });
  });

  // 测试 17: 只有图片（无文本）使用默认占位符
  it('17: images only (no text) gets default [图片] placeholder', () => {
    const attachments = [
      { category: 'image', base64: 'data:image/png;base64,abc123' },
      { category: 'image', base64: 'data:image/jpeg;base64,xyz789' },
    ];
    const result = buildUserMessageContent('', attachments);
    expect(Array.isArray(result)).toBe(true);
    expect(result[0]).toEqual({ type: 'text', text: '[图片]' });
    expect(result[1]).toEqual({
      type: 'image_url',
      image_url: { url: 'data:image/png;base64,abc123' },
    });
    expect(result[2]).toEqual({
      type: 'image_url',
      image_url: { url: 'data:image/jpeg;base64,xyz789' },
    });
  });

  // 测试 18: 无 base64 的图片被跳过
  it('18: image without base64 is skipped', () => {
    const attachments = [
      { category: 'image', name: 'missing.png' }, // 无 base64
      { category: 'image', base64: 'data:image/png;base64,abc123' },
    ];
    const result = buildUserMessageContent('Images', attachments);
    expect(result).toEqual([
      { type: 'text', text: 'Images' },
      { type: 'image_url', image_url: { url: 'data:image/png;base64,abc123' } },
    ]);
  });

  // 测试 19: 未就绪的二进制附件被跳过
  it('19: binary attachment not ready is skipped', () => {
    const attachments = [
      {
        category: 'binary',
        name: 'processing.pdf',
        status: 'processing',
        markdown: '# Content',
      },
    ];
    const result = buildUserMessageContent('Hello', attachments);
    expect(result).toBe('Hello');
  });

  // 测试 20: 无语言的文本文件使用通用代码块
  it('20: text file without language uses generic code fence', () => {
    const attachments = [
      {
        category: 'text',
        name: 'unknown.txt',
        textContent: 'plain text content',
        language: '',
      },
    ];
    const result = buildUserMessageContent('File content', attachments);
    expect(result).toBe(
      'File content\n\n--- 附件: unknown.txt ---\n```\nplain text content\n```\n---'
    );
  });

  // 测试 21: 仅空白字符的文本带图片仍获得占位符
  it('21: whitespace-only text with images gets placeholder', () => {
    const attachments = [
      { category: 'image', base64: 'data:image/png;base64,abc123' },
    ];
    const result = buildUserMessageContent('   ', attachments);
    expect(result[0]).toEqual({ type: 'text', text: '[图片]' });
  });
});

// ============================================================
// stripAttachmentsForPersistence 测试组
// ============================================================

describe('stripAttachmentsForPersistence', () => {
  // 测试 18: 字符串原样返回
  it('18: string returns unchanged', () => {
    const result = stripAttachmentsForPersistence('Plain text message');
    expect(result).toBe('Plain text message');
  });

  // 测试 19: 非数组（null、number）返回字符串
  it('19: non-array returns string', () => {
    expect(stripAttachmentsForPersistence(null as any)).toBe('');
    expect(stripAttachmentsForPersistence(42 as any)).toBe('42');
    expect(stripAttachmentsForPersistence(undefined as any)).toBe('');
  });

  // 测试 20: 带文本块的数组提取文本并替换附件块
  it('20: array with text parts extracts text and replaces attachment blocks', () => {
    const content = [
      { type: 'text', text: 'User message\n\n--- 附件: file.js ---\n```javascript\nconst x = 1;\n```\n---' },
      { type: 'image_url', image_url: { url: 'data:image/png;base64,abc' } },
    ];
    const result = stripAttachmentsForPersistence(content);
    expect(result).toBe('User message\n\n[附件: file.js]');
  });

  // 测试 21: 非文本块（图片）被过滤掉
  it('21: non-text parts are filtered out', () => {
    const content = [
      { type: 'text', text: 'Text message' },
      { type: 'image_url', image_url: { url: 'data:image/png;base64,abc' } },
      { type: 'image', source: { type: 'base64', media_type: 'image/png', data: 'abc' } },
    ];
    const result = stripAttachmentsForPersistence(content);
    expect(result).toBe('Text message');
  });

  // 测试 22: 空文本块被过滤掉
  it('22: empty text parts are filtered out', () => {
    const content = [
      { type: 'text', text: '' },
      { type: 'text', text: null },
      { type: 'text', text: 'Valid text' },
    ];
    const result = stripAttachmentsForPersistence(content);
    expect(result).toBe('Valid text');
  });

  // 测试 23: 多个文本块用换行符连接
  it('23: multiple text parts are joined with newline', () => {
    const content = [
      { type: 'text', text: 'First part' },
      { type: 'text', text: 'Second part' },
      { type: 'text', text: 'Third part' },
    ];
    const result = stripAttachmentsForPersistence(content);
    expect(result).toBe('First part\nSecond part\nThird part');
  });

  // 测试 24: 带多行内容的附件模式被替换
  it('24: attachment pattern with multi-line content is replaced', () => {
    const content = [
      {
        type: 'text',
        text: 'Message\n--- 附件: report.md ---\n# Long Report\n\nLine 1\nLine 2\nLine 3\n---',
      },
    ];
    const result = stripAttachmentsForPersistence(content);
    expect(result).toBe('Message\n[附件: report.md]');
  });
});

// ============================================================
// extractTextFromContent 测试组
// ============================================================

describe('extractTextFromContent', () => {
  // 测试 22: 字符串原样返回
  it('22: string returns unchanged', () => {
    const result = extractTextFromContent('Plain text');
    expect(result).toBe('Plain text');
  });

  // 测试 23: 数组从文本块提取文本并用换行符连接
  it('23: array joins text from text parts with newline', () => {
    const content = [
      { type: 'text', text: 'First line' },
      { type: 'image_url', image_url: { url: 'data:image/png;base64,abc' } },
      { type: 'text', text: 'Second line' },
    ];
    const result = extractTextFromContent(content);
    expect(result).toBe('First line\nSecond line');
  });

  // 测试 24: null/undefined 返回空字符串（通过 content ?? ''）
  it('24: null/undefined returns empty string', () => {
    expect(extractTextFromContent(null)).toBe('');
    expect(extractTextFromContent(undefined)).toBe('');
  });

  // 测试 25: 空数组返回空字符串
  it('25: empty array returns empty string', () => {
    const result = extractTextFromContent([]);
    expect(result).toBe('');
  });

  // 测试 26: 带空文本的文本块被视为空
  it('26: text parts with null/undefined text are treated as empty', () => {
    const content = [
      { type: 'text', text: 'Valid' },
      { type: 'text', text: null },
      { type: 'text', text: undefined },
      { type: 'text', text: 'Also valid' },
    ];
    const result = extractTextFromContent(content);
    // 4 个块用 '\n' 连接得到 3 个换行符：Valid\n\n\nAlso valid
    expect(result).toBe('Valid\n\n\nAlso valid');
  });

  // 测试 27: 数字输入返回字符串表示
  it('27: number input returns string representation', () => {
    const result = extractTextFromContent(42 as any);
    expect(result).toBe('42');
  });
});

// ============================================================
// extractImageUrlsFromContent 测试组
// ============================================================

describe('extractImageUrlsFromContent', () => {
  // 测试 25: 字符串返回空数组
  it('25: string returns empty array', () => {
    const result = extractImageUrlsFromContent('Text message');
    expect(result).toEqual([]);
  });

  // 测试 26: 数组从 image_url 块提取 URL
  it('26: array extracts URLs from image_url parts', () => {
    const content = [
      { type: 'text', text: 'Text' },
      { type: 'image_url', image_url: { url: 'data:image/png;base64,abc' } },
      { type: 'image_url', image_url: 'data:image/jpeg;base64,xyz' },
    ];
    const result = extractImageUrlsFromContent(content);
    expect(result).toEqual(['data:image/png;base64,abc', 'data:image/jpeg;base64,xyz']);
  });

  // 测试 27: 过滤掉空 URL
  it('27: filters out empty URLs', () => {
    const content = [
      { type: 'image_url', image_url: { url: 'data:image/png;base64,abc' } },
      { type: 'image_url', image_url: { url: '' } },
      { type: 'image_url', image_url: null },
      { type: 'image_url', image_url: '' },
    ];
    const result = extractImageUrlsFromContent(content);
    expect(result).toEqual(['data:image/png;base64,abc']);
  });

  // 测试 28: 非数组内容返回空数组
  it('28: non-array content returns empty array', () => {
    expect(extractImageUrlsFromContent(null as any)).toEqual([]);
    expect(extractImageUrlsFromContent(42 as any)).toEqual([]);
    expect(extractImageUrlsFromContent({} as any)).toEqual([]);
  });

  // 测试 29: 忽略非 image_url 块，包括 Anthropic 图片格式
  it('29: ignores non-image_url parts including Anthropic image format', () => {
    const content = [
      { type: 'text', text: 'Text' },
      { type: 'image', source: { type: 'base64', media_type: 'image/png', data: 'abc' } },
      { type: 'unknown', data: 'something' },
    ];
    const result = extractImageUrlsFromContent(content);
    expect(result).toEqual([]);
  });
});

// ============================================================
// buildAnthropicSystemPrompt 测试组
// ============================================================

describe('buildAnthropicSystemPrompt', () => {
  // 测试 28: null/undefined/'' 返回 undefined
  it('28: null/undefined/empty string returns undefined', () => {
    expect(buildAnthropicSystemPrompt(null)).toBeUndefined();
    expect(buildAnthropicSystemPrompt(undefined)).toBeUndefined();
    expect(buildAnthropicSystemPrompt('')).toBeUndefined();
  });

  // 测试 29: 字符串返回带 cache_control 的数组
  it('29: string returns array with cache_control', () => {
    const result = buildAnthropicSystemPrompt('You are a helpful assistant.');
    expect(result).toEqual([
      {
        type: 'text',
        text: 'You are a helpful assistant.',
        cache_control: { type: 'ephemeral' },
      },
    ]);
  });

  // 测试 30: 数组原样返回
  it('30: array returns as-is', () => {
    const input = [
      { type: 'text', text: 'System prompt', cache_control: { type: 'ephemeral' } },
      { type: 'text', text: 'Additional text' },
    ];
    const result = buildAnthropicSystemPrompt(input);
    expect(result).toBe(input);
  });

  // 测试 31: 0（falsy）由于 !systemContent 检查返回 undefined
  it('31: zero (falsy) returns undefined due to !systemContent check', () => {
    const result = buildAnthropicSystemPrompt(0 as any);
    expect(result).toBeUndefined();
  });
});

// ============================================================
// generateMessageId 测试组（第一阶段：内联回复）
// ============================================================

describe('generateMessageId', () => {
  // 测试 32: 返回字符串
  it('32: returns a string', () => {
    const id = generateMessageId();
    expect(typeof id).toBe('string');
  });

  // 测试 33: 生成唯一 ID
  it('33: generates unique IDs for different calls', () => {
    const id1 = generateMessageId();
    const id2 = generateMessageId();
    expect(id1).not.toBe(id2);
  });

  // 测试 34: ID 非空
  it('34: returns non-empty string', () => {
    const id = generateMessageId();
    expect(id.length).toBeGreaterThan(0);
  });

  // 测试 35: ID 只包含字母数字字符
  it('35: contains only alphanumeric characters (base36)', () => {
    const id = generateMessageId();
    expect(id).toMatch(/^[a-z0-9]+$/);
  });

  // 测试 36: 1000 次生成无冲突
  it('36: no collision for 1000 generations', () => {
    const ids = new Set();
    for (let i = 0; i < 1000; i++) {
      const id = generateMessageId();
      if (ids.has(id)) {
        expect.fail(`Collision detected: ${id}`);
      }
      ids.add(id);
    }
    expect(ids.size).toBe(1000);
  });
});

// ============================================================
// ensureMessageIds 测试组（第一阶段：内联回复）
// ============================================================

describe('ensureMessageIds', () => {
  // 测试 37: 为无 ID 的消息添加 ID
  it('37: adds ID to message without ID', () => {
    const messages = [
      { role: 'user', content: 'Hello' },
    ];
    const result = ensureMessageIds(messages);
    expect(result[0].id).toBeDefined();
    expect(typeof result[0].id).toBe('string');
  });

  // 测试 38: 保留现有消息 ID
  it('38: preserves existing message ID', () => {
    const existingId = 'msg-123';
    const messages = [
      { id: existingId, role: 'user', content: 'Hello' },
    ];
    const result = ensureMessageIds(messages);
    expect(result[0].id).toBe(existingId);
  });

  // 测试 39: 为无 threads 的消息添加 threads 数组
  it('39: adds threads array to message without threads', () => {
    const messages = [
      { id: 'm1', role: 'user', content: 'Hello' },
    ];
    const result = ensureMessageIds(messages);
    expect(result[0].threads).toBeDefined();
    expect(Array.isArray(result[0].threads)).toBe(true);
  });

  // 测试 40: 保留现有 threads 数组
  it('40: preserves existing threads array', () => {
    const existingThread = { id: 't1', anchorFingerprint: 'fp1', anchorText: 'text', orphaned: false, messages: [] };
    const messages = [
      { id: 'm1', role: 'user', content: 'Hello', threads: [existingThread] },
    ];
    const result = ensureMessageIds(messages);
    expect(result[0].threads).toBe(messages[0].threads); // 同一引用
  });

  // 测试 41: 为无 ID 的线程消息添加 ID
  it('41: adds IDs to thread messages without IDs', () => {
    const messages = [
      {
        id: 'm1',
        role: 'user',
        content: 'Hello',
        threads: [
          {
            id: 't1',
            anchorFingerprint: 'fp1',
            anchorText: 'text',
            orphaned: false,
            messages: [
              { role: 'user', content: 'Why?' }, // 无 ID
              { role: 'assistant', content: 'Because' }, // 无 ID
            ],
          },
        ],
      },
    ];
    const result = ensureMessageIds(messages);
    expect(result[0].threads[0].messages[0].id).toBeDefined();
    expect(result[0].threads[0].messages[1].id).toBeDefined();
  });

  // 测试 42: 保留现有线程消息 ID
  it('42: preserves existing thread message IDs', () => {
    const existingMsgId = 'thread-msg-123';
    const messages = [
      {
        id: 'm1',
        role: 'user',
        content: 'Hello',
        threads: [
          {
            id: 't1',
            anchorFingerprint: 'fp1',
            anchorText: 'text',
            orphaned: false,
            messages: [
              { id: existingMsgId, role: 'user', content: 'Why?' },
            ],
          },
        ],
      },
    ];
    const result = ensureMessageIds(messages);
    expect(result[0].threads[0].messages[0].id).toBe(existingMsgId);
  });

  // 测试 43: 处理空消息数组
  it('43: handles empty messages array', () => {
    const result = ensureMessageIds([]);
    expect(result).toEqual([]);
  });

  // 测试 44: 处理带 null threads 的消息
  it('44: handles messages with null threads', () => {
    const messages = [
      { id: 'm1', role: 'user', content: 'Hello', threads: null },
    ];
    const result = ensureMessageIds(messages);
    expect(result[0].threads).toEqual([]);
  });

  // 测试 45: 返回新数组（不可变）
  it('45: returns new array (immutable update)', () => {
    const messages = [
      { id: 'm1', role: 'user', content: 'Hello' },
    ];
    const result = ensureMessageIds(messages);
    expect(result).not.toBe(messages);
  });

  // 测试 46: 对无 ID 的消息使用定向展开
  it('46: uses targeted spread - preserves message references with existing IDs', () => {
    const thread = { id: 't1', anchorFingerprint: 'fp1', anchorText: 'text', orphaned: false, messages: [] };
    const messages = [
      { id: 'm1', role: 'user', content: 'Hello', threads: [thread] },
      { id: 'm2', role: 'assistant', content: 'Hi', threads: [] },
    ];
    const result = ensureMessageIds(messages);

    // 由于所有消息都有 ID，它们应该有相同的引用
    //（除了线程处理创建的新数组）
    expect(result[0].id).toBe(messages[0].id);
    expect(result[1].id).toBe(messages[1].id);
  });
});
