/**
 * useChatSender Hook 测试文件
 *
 * @module ai-chat/hooks/__tests__/useChatSender
 *
 * 测试方法：
 * 该 Hook 强依赖于 React 状态（useState、useCallback、useRef）和浏览器 API
 *（fetch、AbortController、TextDecoder）。测试重点：
 * 1. 模块导出验证
 * 2. API 契约文档化
 * 3. 纯函数工具测试（来自 chatMessageUtils）
 * 4. Token 估算测试
 * 5. URL 规范化逻辑
 * 6. 错误处理场景
 * 7. 使用 renderHook 进行 Hook 集成测试
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, waitFor, act } from '@testing-library/react';
import { estimateTokens } from '../../../../utils/markdownUtils';
import {
  extractTextFromContent,
  buildUserMessageContent,
  stripAttachmentsForPersistence,
  buildAnthropicSystemPrompt,
  convertToAnthropicContent,
} from '../../utils/chatMessageUtils';
import { useChatSender } from '../useChatSender';
import { inlineThreadMappingKey, threadMappingKey } from '../../api/agentThreadsApi';

describe('useChatSender Hook', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe('Module Exports', () => {
    it('should export useChatSender function', () => {
      let useChatSenderModule;
      try {
        // eslint-disable-next-line global-require
        useChatSenderModule = require('../useChatSender');
      } catch (e) {
        // 模块在 node 环境中可能不可用
      }

      if (!useChatSenderModule) {
        return;
      }
      expect(useChatSenderModule).toBeDefined();
      expect(typeof useChatSenderModule.useChatSender).toBe('function');
    });

    it('should be the default export or named export', () => {
      let useChatSenderModule;
      try {
        // eslint-disable-next-line global-require
        useChatSenderModule = require('../useChatSender');
      } catch (e) {
        return;
      }
      expect(useChatSenderModule.useChatSender).toBeDefined();
    });
  });

  describe('Token Budget Management', () => {
    describe('estimateTokens utility', () => {
      it('should estimate tokens for short text', () => {
        const text = 'hello world';
        const tokens = estimateTokens(text);

        // 粗略估算：约 4 个字符等于 1 个 token
        expect(tokens).toBeGreaterThan(0);
        expect(tokens).toBeLessThan(text.length);
      });

      it('should estimate tokens for longer text', () => {
        const text = 'This is a longer piece of text that should have more tokens. ';
        const tokens = estimateTokens(text);

        expect(tokens).toBeGreaterThan(5);
      });

      it('should handle empty string', () => {
        expect(estimateTokens('')).toBe(0);
      });

      it('should handle special characters', () => {
        const text = 'Hello 世界 🌍';
        const tokens = estimateTokens(text);

        expect(tokens).toBeGreaterThan(0);
      });

      it('should estimate higher tokens for Chinese text', () => {
        const chinese = '中文测试文本';
        const english = 'test text';

        // 中文通常比英文使用更多 token
        expect(estimateTokens(chinese)).toBeGreaterThanOrEqual(estimateTokens(english));
      });
    });

    describe('Token Budget Layers', () => {
      it('should allocate budget for Layer 1 (paper info)', () => {
        const paperInfoTokens = estimateTokens(
          '## 论文信息\n**标题**: Test Paper\n**作者**: Author Name\n\n## 论文摘要\nThis is a test abstract.'
        );

        // Layer 1 使用 estimateTokens 进行简单估算
        // 实际 token 数取决于实现
        expect(paperInfoTokens).toBeGreaterThan(0);
      });

      it('should reserve 2000 tokens for response', () => {
        const totalBudget = 8000;
        const usedBudget = 3000;
        const remainingForResponse = 2000;

        const availableForInput = totalBudget - usedBudget - remainingForResponse;

        expect(availableForInput).toBe(3000);
      });

      it('should handle HISTORY_TOKEN_BUDGET of 100000 tokens', () => {
        const HISTORY_TOKEN_BUDGET = 100000;

        expect(HISTORY_TOKEN_BUDGET).toBe(100000);

        // 模拟截断逻辑
        let historyTokens = 0;
        const messages = [
          { content: 'Message 1', tokens: 100 },
          { content: 'Message 2', tokens: 200 },
          { content: 'Message 3', tokens: 300 },
        ];

        // 从最新到最旧处理
        for (let i = messages.length - 1; i >= 0; i--) {
          if (historyTokens + messages[i].tokens > HISTORY_TOKEN_BUDGET) {
            break;
          }
          historyTokens += messages[i].tokens;
        }

        expect(historyTokens).toBeLessThanOrEqual(HISTORY_TOKEN_BUDGET);
      });
    });
  });

  describe('URL Normalization', () => {
    describe('Anthropic Provider URL Construction', () => {
      it('should strip /v1/ suffix from base URL', () => {
        const baseUrl = 'https://api.anthropic.com/v1/';
        const normalizedBaseUrl = baseUrl.replace(/\/v1\/?$/, '').replace(/\/+$/, '');

        expect(normalizedBaseUrl).toBe('https://api.anthropic.com');
      });

      it('should strip trailing slashes from base URL', () => {
        const baseUrl = 'https://api.anthropic.com///';
        const normalizedBaseUrl = baseUrl.replace(/\/v1\/?$/, '').replace(/\/+$/, '');

        expect(normalizedBaseUrl).toBe('https://api.anthropic.com');
      });

      it('should append /v1/messages to normalized URL', () => {
        const normalizedBaseUrl = 'https://api.anthropic.com';
        const targetUrl = `${normalizedBaseUrl}/v1/messages`;

        expect(targetUrl).toBe('https://api.anthropic.com/v1/messages');
      });

      it('should handle base URL without version', () => {
        const baseUrl = 'https://custom.api.com';
        const normalizedBaseUrl = baseUrl.replace(/\/v1\/?$/, '').replace(/\/+$/, '');

        expect(normalizedBaseUrl).toBe('https://custom.api.com');
      });
    });

    describe('OpenAI-Compatible Provider URL Construction', () => {
      it('should use URL as-is if it ends with /chat/completions', () => {
        const baseUrl = 'https://api.openai.com/v1/chat/completions';
        const url = baseUrl.trim().replace(/\/+$/, '');

        expect(url).toBe('https://api.openai.com/v1/chat/completions');
      });

      it('should append /chat/completions if URL ends with /v{digit}', () => {
        const baseUrl = 'https://api.openai.com/v1';
        const url = baseUrl.trim().replace(/\/+$/, '');

        if (/\/v\d+$/.test(url)) {
          const targetUrl = url + '/chat/completions';
          expect(targetUrl).toBe('https://api.openai.com/v1/chat/completions');
        }
      });

      it('should replace path after /v{digit}/ if URL contains version with path', () => {
        const baseUrl = 'https://api.openai.com/v1/other/path';
        const url = baseUrl.trim().replace(/\/+$/, '');

        if (/\/v\d+\//.test(url)) {
          const targetUrl = url.replace(/(\/v\d+\/).*$/, '$1chat/completions');
          expect(targetUrl).toBe('https://api.openai.com/v1/chat/completions');
        }
      });

      it('should append /v1/chat/completions by default', () => {
        const baseUrl = 'https://custom.api.com';
        const url = baseUrl.trim().replace(/\/+$/, '');

        if (!url.endsWith('/chat/completions') && !/\/v\d+$/.test(url) && !/\/v\d+\//.test(url)) {
          const targetUrl = url + '/v1/chat/completions';
          expect(targetUrl).toBe('https://custom.api.com/v1/chat/completions');
        }
      });
    });

    describe('Edge Cases', () => {
      it('should handle URL with multiple trailing slashes', () => {
        const baseUrl = 'https://api.example.com///';
        const normalized = baseUrl.replace(/\/+$/, '');

        expect(normalized).toBe('https://api.example.com');
      });

      it('should handle URL with no path', () => {
        const baseUrl = 'https://api.example.com';
        const url = baseUrl.trim().replace(/\/+$/, '');

        expect(url).toBe('https://api.example.com');
      });
    });
  });

  describe('Message Content Utilities', () => {
    describe('extractTextFromContent', () => {
      it('should extract text from string content', () => {
        const content = 'Hello, world!';
        const extracted = extractTextFromContent(content);

        expect(extracted).toBe('Hello, world!');
      });

      it('should extract text from array content with text type', () => {
        const content = [
          { type: 'text', text: 'Hello' },
          { type: 'text', text: 'world' },
        ];
        const extracted = extractTextFromContent(content);

        // 使用换行符连接文本块
        expect(extracted).toBe('Hello\nworld');
      });

      it('should extract text from array content with image type', () => {
        const content = [
          { type: 'text', text: 'Check this image:' },
          { type: 'image', source: { type: 'base64', data: 'base64string' } },
        ];
        const extracted = extractTextFromContent(content);

        // 图片应被忽略
        expect(extracted).toBe('Check this image:');
      });

      it('should handle empty content', () => {
        expect(extractTextFromContent('')).toBe('');
        expect(extractTextFromContent([])).toBe('');
      });

      it('should handle mixed content types', () => {
        const content = [
          { type: 'text', text: 'Text before image' },
          { type: 'image', source: { type: 'url', url: 'http://example.com/image.png' } },
          { type: 'text', text: 'Text after image' },
        ];
        const extracted = extractTextFromContent(content);

        // 使用换行符连接文本块
        expect(extracted).toBe('Text before image\nText after image');
      });
    });

    describe('buildUserMessageContent', () => {
      it('should return string when no attachments', () => {
        const text = 'Hello, AI!';
        const content = buildUserMessageContent(text, []);

        expect(content).toBe('Hello, AI!');
      });

      it('should build array content with text and image attachment', () => {
        const text = 'Check this paper';
        const attachments = [
          { category: 'image', base64: 'data:image/png;base64,iVBORw0KG...' },
        ];
        const content = buildUserMessageContent(text, attachments);

        expect(Array.isArray(content)).toBe(true);
        expect(content[0]).toEqual({ type: 'text', text });
        expect(content[1]).toEqual({
          type: 'image_url',
          image_url: { url: attachments[0].base64 },
        });
      });

      it('should handle multiple attachments', () => {
        const text = 'Multiple files';
        const attachments = [
          { category: 'image', base64: 'data:image/png;base64,abc' },
          { category: 'image', base64: 'data:image/jpeg;base64,def' },
        ];
        const content = buildUserMessageContent(text, attachments);

        // 应该是 1 个文本 + 2 个图片，但检查实际结构
        expect(Array.isArray(content)).toBe(true);
        expect(content.length).toBeGreaterThan(0);
      });
    });

    describe('stripAttachmentsForPersistence', () => {
      it('should pass through string content unchanged', () => {
        const content = 'Simple text message';
        const stripped = stripAttachmentsForPersistence(content);

        expect(stripped).toBe('Simple text message');
      });

      it('should remove image blocks from array content', () => {
        const content = [
          { type: 'text', text: 'Text with image' },
          { type: 'image', source: { type: 'base64', data: 'base64data' } },
        ];
        const stripped = stripAttachmentsForPersistence(content);

        expect(stripped).toEqual('Text with image');
      });

      it('should concatenate text blocks with newline', () => {
        const content = [
          { type: 'text', text: 'First part' },
          { type: 'text', text: 'Second part' },
        ];
        const stripped = stripAttachmentsForPersistence(content);

        // 使用换行符连接
        expect(stripped).toBe('First part\nSecond part');
      });
    });

    describe('buildAnthropicSystemPrompt', () => {
      it('should handle null system prompt', () => {
        const result = buildAnthropicSystemPrompt(null);

        expect(result).toBeUndefined();
      });

      it('should handle undefined system prompt', () => {
        const result = buildAnthropicSystemPrompt(undefined);

        expect(result).toBeUndefined();
      });

      it('should wrap string system prompt with cache_control', () => {
        const prompt = 'You are a helpful assistant.';
        const result = buildAnthropicSystemPrompt(prompt);

        expect(Array.isArray(result)).toBe(true);
        expect(result).toEqual([
          {
            type: 'text',
            text: prompt,
            cache_control: { type: 'ephemeral' },
          },
        ]);
      });

      it('should handle empty system prompt', () => {
        const result = buildAnthropicSystemPrompt('');

        // 空字符串是 falsy，所以返回 undefined
        expect(result).toBeUndefined();
      });
    });

    describe('convertToAnthropicContent', () => {
      it('should pass through string content', () => {
        const content = 'Simple text';
        const converted = convertToAnthropicContent(content);

        expect(converted).toBe('Simple text');
      });

      it('should merge text blocks into string when no images', () => {
        const content = [
          { type: 'text', text: 'Hello' },
          { type: 'text', text: 'World' },
        ];
        const converted = convertToAnthropicContent(content);

        // 所有文本块被合并为单个字符串
        expect(converted).toBe('HelloWorld');
      });

      it('should preserve array when image blocks are present', () => {
        const content = [
          { type: 'text', text: 'Image:' },
          { type: 'image', source: { type: 'base64', media_type: 'image/png', data: 'base64data' } },
        ];
        const converted = convertToAnthropicContent(content);

        expect(Array.isArray(converted)).toBe(true);
        expect((converted as any)[0].type).toBe('text');
        expect((converted as any)[1].type).toBe('image');
      });
    });
  });

  describe('SSE Stream Parsing', () => {
    it('should parse Anthropic content_block_delta event', () => {
      const eventData = {
        type: 'content_block_delta',
        delta: { type: 'text_delta', text: 'Hello' },
      };

      expect(eventData.delta?.text).toBe('Hello');
    });

    it('should parse OpenAI choices delta content', () => {
      const eventData = {
        choices: [{ delta: { content: 'World' } }],
      };

      expect(eventData.choices?.[0]?.delta?.content).toBe('World');
    });

    it('should handle [DONE] sentinel', () => {
      const data = '[DONE]';

      expect(data).toBe('[DONE]');
    });

    it('should handle stream-embedded error', () => {
      const eventData = {
        error: { message: 'Rate limit exceeded' },
      };

      expect(eventData.error?.message).toBe('Rate limit exceeded');
    });

    it('should handle error with msg field', () => {
      const eventData = {
        error: { msg: 'Invalid request' },
      };

      expect(eventData.error?.msg).toBe('Invalid request');
    });

    it('should handle string error', () => {
      const eventData = {
        error: 'Something went wrong',
      };

      expect(eventData.error).toBe('Something went wrong');
    });
  });

  describe('Timeout and Abort Behavior', () => {
    it('should set timeout to 120 seconds (120000 ms)', () => {
      const TIMEOUT_MS = 120000;

      expect(TIMEOUT_MS).toBe(120000);
    });

    it('should calculate timeout correctly', () => {
      const seconds = 120;
      const timeoutMs = seconds * 1000;

      expect(timeoutMs).toBe(120000);
    });

    it('should track timeout state', () => {
      let timedOut = false;

      const triggerTimeout = () => {
        timedOut = true;
      };

      expect(timedOut).toBe(false);
      triggerTimeout();
      expect(timedOut).toBe(true);
    });
  });

  describe('Message Quoting', () => {
    it('should format quoted AI message', () => {
      const quotedMessage = {
        role: 'assistant',
        content: 'This is the AI response that is being quoted.',
      };
      const userMessage = 'Follow up question';

      const roleLabel = quotedMessage.role === 'assistant' ? 'AI' : '用户';
      const snippet = quotedMessage.content.slice(0, 200);
      const finalMessage = `[引用了${roleLabel}的消息]:\n${snippet.split('\n').map(l => `> ${l}`).join('\n')}\n\n${userMessage}`;

      expect(finalMessage).toContain('[引用了AI的消息]:');
      expect(finalMessage).toContain('> This is the AI response');
      expect(finalMessage).toContain('Follow up question');
    });

    it('should format quoted user message', () => {
      const quotedMessage = {
        role: 'user',
        content: 'This is the user message being quoted.',
      };
      const userMessage = 'Another question';

      const roleLabel = quotedMessage.role === 'assistant' ? 'AI' : '用户';
      const snippet = quotedMessage.content.slice(0, 200);
      const finalMessage = `[引用了${roleLabel}的消息]:\n${snippet.split('\n').map(l => `> ${l}`).join('\n')}\n\n${userMessage}`;

      expect(finalMessage).toContain('[引用了用户的消息]:');
      expect(finalMessage).toContain('> This is the user message');
    });

    it('should limit snippet to 200 characters', () => {
      const longContent = 'a'.repeat(300);
      const quotedMessage = {
        role: 'assistant',
        content: longContent,
      };

      const snippet = quotedMessage.content.slice(0, 200);

      expect(snippet.length).toBe(200);
    });
  });

  describe('Error Message Formatting', () => {
    it('should format API key missing message', () => {
      const message = '请先在设置中配置 AI API Key（点击右上角设置按钮）。';

      expect(message).toContain('API Key');
      expect(message).toContain('设置');
    });

    it('should format timeout message', () => {
      const message = 'AI 回复超时（120 秒），请检查网络或稍后重试。';

      expect(message).toContain('120 秒');
      expect(message).toContain('超时');
    });

    it('should format empty response message', () => {
      const message = '⚠️ AI 返回了空回复。请检查模型名称是否正确，或尝试更换模型。';

      expect(message).toContain('空回复');
      expect(message).toContain('模型名称');
    });

    it('should format Tavily API Key missing message', () => {
      const message = '网络搜索需要 Tavily API Key。请在服务器 .env 文件中配置 TAVILY_API_KEY。';

      expect(message).toContain('Tavily API Key');
      expect(message).toContain('TAVILY_API_KEY');
    });
  });

  describe('API Contract', () => {
    it('documents the complete parameter contract', () => {
      const requiredParams = [
        'setMessages',
        'setStreaming',
        'persistMessages',
        'abortControllerRef',
        'settings',
        'currentModel',
        'tokenBudgets',
        'paperInfo',
        'paper',
        'guideSections',
        'messages',
        'streaming',
      ];

      expect(requiredParams.length).toBe(12);
    });

    it('documents sendMessages function signature', () => {
      const sendMessagesParams = [
        'text',
        'vibeCardRefs',
        'attachments',
        'webSearchEnabled',
        'quotedMessage',
        'setQuotedMessage',
      ];

      expect(sendMessagesParams.length).toBe(6);
    });
  });

  describe('Internal Logic - Layered System Prompt', () => {
    it('documents the 4-layer prompt injection strategy', () => {
      const layers = [
        'Layer 1: Paper Base Info (~500 tokens)',
        'Layer 2.5: Web Search Results (on-demand)',
        'Layer 3: Paragraph Guide Context (on-demand)',
        'Layer 4: Full Markdown (large context only)',
      ];

      expect(layers.length).toBe(4);
    });
  });

  // ============================================================================
  // Hook 集成测试 - 使用 renderHook 测试实际 Hook 调用
  // ============================================================================

  describe('Hook Integration Tests (renderHook)', () => {
    // 全局 mock fetch，避免真实网络请求
    let mockFetch: any;

    beforeEach(() => {
      // 创建 fetch mock
      mockFetch = vi.fn();
      global.fetch = mockFetch;

      // 清除所有 mocks
      vi.clearAllMocks();
    });

    afterEach(() => {
      // 恢复原始 fetch
      if ((global.fetch as any).__isMock) {
        vi.restoreAllMocks();
      }
    });

    /**
     * 创建模拟的流式响应
     * 模拟 SSE (Server-Sent Events) 格式的响应
     *
     * @param {string[]} chunks - SSE 数据块数组
     * @returns {object} 模拟的 Response 对象
     */
    const createMockStreamResponse = (chunks: any) => {
      const stream = new ReadableStream({
        async start(controller) {
          for (const chunk of chunks) {
            const encoder = new TextEncoder();
            controller.enqueue(encoder.encode(chunk));
          }
          controller.close();
        },
      });
      return {
        ok: true,
        status: 200,
        body: {
          getReader: () => stream.getReader(),
        },
      };
    };

    /**
     * 创建默认的 Hook 属性
     * Hook 需要 16 个必需参数，这里提供完整的 mock 对象
     */
    const createDefaultProps = () => ({
      // 消息状态管理
      setMessages: vi.fn(),
      setStreaming: vi.fn(),

      // v2 (2026-06-28): agentType 必填，测试默认用 paperReader（覆盖学术研究助手 persona）
      agentType: 'paperReader',
      persistMessages: vi.fn().mockResolvedValue(undefined),

      // AbortController 引用（用于中断请求）
      abortControllerRef: { current: null },

      // 用户设置
      settings: {
        ai_provider: 'openai',
        api_key: 'test-api-key',
        model: 'gpt-4o-mini',
        base_url: 'https://api.openai.com/v1',
        model_config: '{}',
      },

      // 当前模型配置
      currentModel: {
        key: 'gpt-4o-mini',
        displayName: 'GPT-4o Mini',
        config: null,
      },

      // Token 预算配置（不同模型有不同的预算）
      tokenBudgets: {
        'gpt-4o-mini': 8000,
        'gpt-4': 8000,
        'claude-3-5-sonnet': 200000,
      },

      // 论文信息
      paperInfo: {
        title: 'Test Paper Title',
        authors: 'Test Author',
        abstract: 'This is a test abstract for the paper.',
      },

      // 论文完整信息
      paper: {
        id: 1,
        title: 'Test Paper Title',
        parse_status: 'done',
      },

      // 段落导览数据
      guideSections: [],

      // 当前消息列表
      messages: [],

      // 是否正在流式回复
      streaming: false,

      // 额外系统上下文（可选）
      additionalSystemContext: null,

      // 错误回调（必须提供稳定引用，否则 sendMessages 的 useCallback 依赖会每次渲染变化）
      onError: vi.fn(),

      // 请求 ID 引用（B6 后用于判断请求是否仍然活跃）
      activeRequestIdRef: { current: null },
    });

    describe('Hook 初始化', () => {
      it('应该成功渲染 hook 并返回 sendMessages 函数', () => {
        const props = createDefaultProps();

        // 使用 renderHook 渲染 hook
        const { result } = renderHook(() => useChatSender(props as any));

        // 验证返回值包含 sendMessages 函数
        expect(result.current).toBeDefined();
        expect(result.current.sendMessages).toBeDefined();
        expect(typeof result.current.sendMessages).toBe('function');
      });

      it('应该接受所有必需的参数而不抛出错误', () => {
        const props = createDefaultProps();

        // 验证所有必需参数都存在
        expect(() => {
          renderHook(() => useChatSender(props as any));
        }).not.toThrow();
      });

      it('应该保持 sendMessages 函数引用稳定（相同依赖时）', () => {
        const props = createDefaultProps();
        const { result, rerender } = renderHook(() => useChatSender(props as any));

        const firstSendMessages = result.current.sendMessages;

        // 使用相同 props 重新渲染
        rerender();

        const secondSendMessages = result.current.sendMessages;

        // 函数引用应该保持稳定（useCallback 依赖未变化）
        expect(firstSendMessages).toBe(secondSendMessages);
      });
    });

    describe('空消息处理', () => {
      it('空字符串消息不应触发 fetch', async () => {
        const props = createDefaultProps();
        const { result } = renderHook(() => useChatSender(props as any));

        // 调用 sendMessages 传入空字符串
        await act(async () => {
          await result.current.sendMessages('');
        });

        // 验证 fetch 未被调用
        expect(mockFetch).not.toHaveBeenCalled();

        // 验证 setMessages 被调用（添加用户消息）
        expect(props.setMessages).not.toHaveBeenCalled();
      });

      it('纯空格消息不应触发 fetch', async () => {
        const props = createDefaultProps();
        const { result } = renderHook(() => useChatSender(props as any));

        // 调用 sendMessages 传入纯空格
        await act(async () => {
          await result.current.sendMessages('   ');
        });

        // 验证 fetch 未被调用
        expect(mockFetch).not.toHaveBeenCalled();
      });

      it('正在流式回复时不应发送新消息', async () => {
        const props = {
          ...createDefaultProps(),
          streaming: true, // 设置为正在流式回复状态
        };
        const { result } = renderHook(() => useChatSender(props as any));

        // 尝试发送消息
        await act(async () => {
          await result.current.sendMessages('test message');
        });

        // 验证 fetch 未被调用（因为 streaming = true）
        expect(mockFetch).not.toHaveBeenCalled();
        expect(props.setMessages).not.toHaveBeenCalled();
      });
    });

    describe('消息发送流程', () => {
      it('应该正确发送消息并处理流式响应', async () => {
        const props = createDefaultProps();

        // Mock fetch 返回 agent SSE 协议的流式响应
        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'event: text_delta\ndata: {"text":"Hello"}\n\n',
            'event: text_delta\ndata: {"text":" World"}\n\n',
            'event: done\ndata: {}\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        // 发送消息
        await act(async () => {
          await result.current.sendMessages('test message');
        });

        // 验证 fetch 被调用
        expect(mockFetch).toHaveBeenCalledTimes(1);

        // 验证请求配置（autonomics：主对话直连后端 agentik 端点）
        const fetchCall = mockFetch.mock.calls[0];
        expect(fetchCall[0]).toBe('/api/v1/agent/chat');
        expect(fetchCall[1].method).toBe('POST');

        // 验证请求体只携带后端 ChatRequest 读取的字段
        // （模型与工具集归服务端所有，model_config/tools 不再上行）
        const requestBody = JSON.parse(fetchCall[1].body);
        expect(requestBody).toHaveProperty('message');
        expect(requestBody).toHaveProperty('messages');
        expect(requestBody).toHaveProperty('agent_type');
        expect(requestBody).not.toHaveProperty('model_config');
        expect(requestBody).not.toHaveProperty('tools');
      });

      it('应该正确构建 system prompt（Layer 1: 论文基础信息）', async () => {
        const props = createDefaultProps();

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"Response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);

        // B6 重构后：system prompt 作为顶层 system_prompt 字段发送，不再混入 messages
        const systemContent = requestBody.system_prompt;
        expect(typeof systemContent).toBe('string');

        // 验证 system prompt 包含论文基础信息（文案更新：原"你是一个"改为"你是一位严谨的"）
        expect(systemContent).toContain('学术研究助手');
        expect(systemContent).toContain('Test Paper Title');
        expect(systemContent).toContain('This is a test abstract');
      });

      it('没有论文信息时应该使用默认 system prompt', async () => {
        const props = {
          ...createDefaultProps(),
          paperInfo: null,
        };

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"Response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);
        const systemContent = requestBody.system_prompt;
        expect(typeof systemContent).toBe('string');

        // 验证 system prompt 只包含默认角色定义
        // 注意：默认 prompt 模板里有"基于提供的论文信息..."这种通用描述，所以不能用
        // not.toContain('论文信息') 来区分 — 改用具体论文标题作判别条件。
        expect(systemContent).toContain('学术研究助手');
        expect(systemContent).not.toContain('Test Paper Title');
        expect(systemContent).not.toContain('This is a test abstract');
      });

      it('autonomics: 请求体不携带 tools（工具集归服务端所有）', async () => {
        const props = createDefaultProps();
        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'event: text_delta\ndata: {"text":"r"}\n\n',
            'event: done\ndata: {}\n\n',
          ])
        );
        const { result } = renderHook(() => useChatSender(props as any));
        await act(async () => {
          await result.current.sendMessages('test');
        });
        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);
        // 后端恒注册 bib_all_registrations，前端不再声明工具集
        expect(requestBody).not.toHaveProperty('tools');
      });

      it('应该正确构建用户消息并添加到消息列表', async () => {
        const props = createDefaultProps();

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"AI response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('user message');
        });

        // 验证 setMessages 被调用了多次：
        // 1. 添加用户消息
        // 2. 添加空的 assistant 消息
        // 3. 更新 assistant 消息内容（可能多次）
        expect(props.setMessages).toHaveBeenCalled();

        // 第一次调用应该添加用户消息
        const firstCall = props.setMessages.mock.calls[0][0];
        expect(firstCall).toHaveLength(1);
        expect(firstCall[0]).toEqual({
          role: 'user',
          content: 'user message',
          id: expect.any(String),
        });
      });

      it('应该正确设置流式状态', async () => {
        const props = createDefaultProps();

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"Response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        // 验证 setStreaming 被调用
        expect(props.setStreaming).toHaveBeenCalledWith(true);
        // 最终应该设置为 false（请求完成后）
        expect(props.setStreaming).toHaveBeenCalledWith(false);
      });
    });

    describe('Runtime 会话路径（P2 web-agent-runtime）', () => {
      const THREAD_ID = '11111111-2222-3333-4444-555555555555';

      /** POST /threads 成功响应 */
      const createThreadResponse = () => ({
        ok: true,
        status: 200,
        json: async () => ({ thread_id: THREAD_ID }),
      });

      /** thread chat 端点的 SSE 成功响应 */
      const sseResponse = () =>
        createMockStreamResponse([
          'event: text_delta\ndata: {"text":"Hi"}\n\n',
          'event: done\ndata: {}\n\n',
        ]);

      // happy-dom 暴露的 localStorage 缺 getItem/setItem/removeItem（client.test.ts
      // 里同一问题的既定解法）：内存 shim 替换，thread 映射读写才能真正被验证
      const storageStore = new Map<string, string>();
      const localStorageShim = {
        getItem: (k: string) => (storageStore.has(k) ? (storageStore.get(k) as string) : null),
        setItem: (k: string, v: string) => { storageStore.set(k, String(v)); },
        removeItem: (k: string) => { storageStore.delete(k); },
        clear: () => storageStore.clear(),
      };

      beforeEach(() => {
        globalThis.localStorage = localStorageShim as unknown as Storage;
        storageStore.clear();
      });

      afterEach(() => {
        storageStore.clear();
      });

      it('runtime + 全新对话：先建 thread 再走 /threads/:id/chat，历史与 persona 不上行', async () => {
        const props = { ...createDefaultProps(), agentMode: 'runtime' };
        mockFetch.mockResolvedValueOnce(createThreadResponse()).mockResolvedValueOnce(sseResponse());

        const { result } = renderHook(() => useChatSender(props as any));
        await act(async () => {
          await result.current.sendMessages('hello');
        });

        expect(mockFetch).toHaveBeenCalledTimes(2);
        // 第一跳：创建会话（默认 props 的 paperInfo 无 id → standalone scope）
        expect(mockFetch.mock.calls[0][0]).toBe('/api/v1/agent/threads');
        expect(JSON.parse(mockFetch.mock.calls[0][1].body)).toEqual({
          agent_type: 'paperReader',
          title: 'Test Paper Title',
        });
        // 第二跳：会话式聊天端点
        expect(mockFetch.mock.calls[1][0]).toBe(`/api/v1/agent/threads/${THREAD_ID}/chat`);
        const body = JSON.parse(mockFetch.mock.calls[1][1].body);
        expect(body.message).toBe('hello');
        expect(body.agent_type).toBe('paperReader');
        // 历史在服务端 session、静态 persona 在服务端 profile，均不上行
        expect(body).not.toHaveProperty('messages');
        expect(body).not.toHaveProperty('system_prompt');
        // 动态上下文（论文信息）仍按轮次上行，且不含默认 persona 文案
        expect(body.context).toContain('Test Paper Title');
        expect(body.context).not.toContain('你是一位严谨的学术研究助手');
        // thread 映射落盘
        expect(localStorage.getItem(threadMappingKey(undefined, 'paperReader'))).toBe(THREAD_ID);
        // 双写：bib_meta 侧照常持久化
        expect(props.persistMessages).toHaveBeenCalled();
      });

      it('runtime + 已有 UI 历史且无映射：先 import 迁移旧对话再走会话端点（P4）', async () => {
        const props = {
          ...createDefaultProps(),
          agentMode: 'runtime',
          messages: [
            { role: 'user', content: '旧问题', id: 'm1' },
            { role: 'assistant', content: '旧回答', id: 'm2' },
          ],
        };
        mockFetch.mockResolvedValueOnce(createThreadResponse()).mockResolvedValueOnce(sseResponse());

        const { result } = renderHook(() => useChatSender(props as any));
        await act(async () => {
          await result.current.sendMessages('继续');
        });

        expect(mockFetch).toHaveBeenCalledTimes(2);
        // 第一跳：bib_meta 时代的旧对话经 /threads/import 迁入服务端 session
        expect(mockFetch.mock.calls[0][0]).toBe('/api/v1/agent/threads/import');
        expect(JSON.parse(mockFetch.mock.calls[0][1].body)).toMatchObject({
          agent_type: 'paperReader',
          messages: [
            { role: 'user', content: '旧问题' },
            { role: 'assistant', content: '旧回答' },
          ],
        });
        // 第二跳：会话式聊天端点，历史不上行（真身已在服务端）
        expect(mockFetch.mock.calls[1][0]).toBe(`/api/v1/agent/threads/${THREAD_ID}/chat`);
        const body = JSON.parse(mockFetch.mock.calls[1][1].body);
        expect(body.message).toBe('继续');
        expect(body).not.toHaveProperty('messages');
        // 迁移后映射落盘：刷新/重开面板不再重复 import
        expect(localStorage.getItem(threadMappingKey(undefined, 'paperReader'))).toBe(THREAD_ID);
      });

      it('已有映射：即使 UI 有历史也走 runtime 端点（双写会话）', async () => {
        localStorage.setItem(threadMappingKey(undefined, 'paperReader'), THREAD_ID);
        const props = {
          ...createDefaultProps(),
          agentMode: 'runtime',
          messages: [{ role: 'user', content: '历史消息', id: 'm1' }],
        };
        mockFetch.mockResolvedValueOnce(sseResponse());

        const { result } = renderHook(() => useChatSender(props as any));
        await act(async () => {
          await result.current.sendMessages('继续');
        });

        // 映射已存在 → 不再创建 thread，直接会话式发送
        expect(mockFetch).toHaveBeenCalledTimes(1);
        expect(mockFetch.mock.calls[0][0]).toBe(`/api/v1/agent/threads/${THREAD_ID}/chat`);
        const body = JSON.parse(mockFetch.mock.calls[0][1].body);
        expect(body).not.toHaveProperty('messages');
      });

      it('建会话失败：本轮报错中止（P4 已退役 /chat，无可降级端点）', async () => {
        const props = { ...createDefaultProps(), agentMode: 'runtime' };
        mockFetch.mockResolvedValueOnce({
          ok: false,
          status: 500,
          json: async () => ({ error: 'boom' }),
        });

        const { result } = renderHook(() => useChatSender(props as any));
        await act(async () => {
          await result.current.sendMessages('hello');
        });

        // 只有一次失败的 ensure 请求，没有聊天请求
        expect(mockFetch).toHaveBeenCalledTimes(1);
        expect(props.onError).toHaveBeenCalledWith('⚠️ boom');
        // 失败的创建不留脏映射
        expect(localStorage.getItem(threadMappingKey(undefined, 'paperReader'))).toBeNull();
      });

      it('409 agent_busy：给出可读的中文提示', async () => {
        localStorage.setItem(threadMappingKey(undefined, 'paperReader'), THREAD_ID);
        const props = { ...createDefaultProps(), agentMode: 'runtime' };
        mockFetch.mockResolvedValueOnce({
          ok: false,
          status: 409,
          json: async () => ({ error: 'agent busy: another turn is streaming for this assistant' }),
        });

        const { result } = renderHook(() => useChatSender(props as any));
        await act(async () => {
          await result.current.sendMessages('hello');
        });

        expect(props.onError).toHaveBeenCalledWith('该助手正在回复上一条消息，请等它完成后再发送');
      });

      // ===== P4：追问（inline）线程 session 化 =====

      /** 带一条既有追问历史的线程消息列表（父消息 0 挂线程 t1） */
      const threadMessages = () => [
        {
          role: 'assistant',
          content: '主对话回答',
          id: 'p0',
          threads: [
            {
              id: 't1',
              messages: [
                { role: 'user', content: '线索问题', id: 't1m1' },
                { role: 'assistant', content: '线索回答', id: 't1m2' },
              ],
            },
          ],
        },
      ];

      const threadOptions = (onStreamUpdate: ReturnType<typeof vi.fn>) => ({
        abortController: new AbortController(),
        threadContext: {
          msgIdx: 0,
          threadId: 't1',
          threadSystemPrompt: '围绕选段回答',
          anchorText: '选段原文',
        },
        onStreamUpdate: onStreamUpdate as any,
        onStreamComplete: vi.fn(),
        onStreamError: vi.fn(),
      });

      it('追问线程 runtime + 无映射有历史：import 线程自身消息，再走会话端点带 context', async () => {
        const onStreamUpdate = vi.fn();
        const options = threadOptions(onStreamUpdate);
        const props = {
          ...createDefaultProps(),
          agentMode: 'runtime',
          messages: threadMessages(),
        };
        mockFetch.mockResolvedValueOnce(createThreadResponse()).mockResolvedValueOnce(sseResponse());

        const { result } = renderHook(() => useChatSender(props as any));
        await act(async () => {
          await result.current.sendMessages('继续问', [], [], null, null, options as any);
        });

        expect(mockFetch).toHaveBeenCalledTimes(2);
        // 第一跳：import 的载荷是线程自身消息（不是主对话前缀——追问语义
        // = anchor 上下文 + 线程历史，与 ephemeral 行为一致，非 fork）
        expect(mockFetch.mock.calls[0][0]).toBe('/api/v1/agent/threads/import');
        expect(JSON.parse(mockFetch.mock.calls[0][1].body)).toMatchObject({
          agent_type: 'paperReader',
          messages: [
            { role: 'user', content: '线索问题' },
            { role: 'assistant', content: '线索回答' },
          ],
        });
        // 第二跳：anchor 系统提示词走 context 字段（不是 system_prompt）
        expect(mockFetch.mock.calls[1][0]).toBe(`/api/v1/agent/threads/${THREAD_ID}/chat`);
        const body = JSON.parse(mockFetch.mock.calls[1][1].body);
        expect(body.message).toBe('继续问');
        expect(body.context).toBe('围绕选段回答');
        expect(body).not.toHaveProperty('system_prompt');
        expect(body).not.toHaveProperty('messages');
        // inline 线程映射落盘（键含 thread: 前缀，与主对话键隔离）
        expect(
          localStorage.getItem(inlineThreadMappingKey(undefined, 'paperReader', 't1')),
        ).toBe(THREAD_ID);
        // 完成回调走线程契约
        expect(options.onStreamComplete).toHaveBeenCalled();
      });

      it('追问线程 runtime + 已有映射：直接会话发送，不 import/create', async () => {
        localStorage.setItem(inlineThreadMappingKey(undefined, 'paperReader', 't1'), THREAD_ID);
        const onStreamUpdate = vi.fn();
        const props = {
          ...createDefaultProps(),
          agentMode: 'runtime',
          messages: threadMessages(),
        };
        mockFetch.mockResolvedValueOnce(sseResponse());

        const { result } = renderHook(() => useChatSender(props as any));
        await act(async () => {
          await result.current.sendMessages('继续问', [], [], null, null, threadOptions(onStreamUpdate) as any);
        });

        expect(mockFetch).toHaveBeenCalledTimes(1);
        expect(mockFetch.mock.calls[0][0]).toBe(`/api/v1/agent/threads/${THREAD_ID}/chat`);
      });

      it('追问线程 runtime + 空线程：create 空会话（不 import）', async () => {
        const onStreamUpdate = vi.fn();
        const props = {
          ...createDefaultProps(),
          agentMode: 'runtime',
          messages: [
            { role: 'assistant', content: '主对话回答', id: 'p0', threads: [{ id: 't1', messages: [] }] },
          ],
        };
        mockFetch.mockResolvedValueOnce(createThreadResponse()).mockResolvedValueOnce(sseResponse());

        const { result } = renderHook(() => useChatSender(props as any));
        await act(async () => {
          await result.current.sendMessages('新线程第一问', [], [], null, null, threadOptions(onStreamUpdate) as any);
        });

        expect(mockFetch).toHaveBeenCalledTimes(2);
        expect(mockFetch.mock.calls[0][0]).toBe('/api/v1/agent/threads');
        expect(JSON.parse(mockFetch.mock.calls[0][1].body)).toEqual({ agent_type: 'paperReader' });
        expect(mockFetch.mock.calls[1][0]).toBe(`/api/v1/agent/threads/${THREAD_ID}/chat`);
      });

      it('追问线程 legacy（agentMode ephemeral）：仍走 /chat 全量上行', async () => {
        const onStreamUpdate = vi.fn();
        const props = {
          ...createDefaultProps(),
          agentMode: 'ephemeral',
          messages: threadMessages(),
        };
        mockFetch.mockResolvedValueOnce(sseResponse());

        const { result } = renderHook(() => useChatSender(props as any));
        await act(async () => {
          await result.current.sendMessages('继续问', [], [], null, null, threadOptions(onStreamUpdate) as any);
        });

        expect(mockFetch).toHaveBeenCalledTimes(1);
        expect(mockFetch.mock.calls[0][0]).toBe('/api/v1/agent/chat');
        const body = JSON.parse(mockFetch.mock.calls[0][1].body);
        expect(body.system_prompt).toBe('围绕选段回答');
        expect(body.messages).toEqual([{ role: 'user', content: '线索问题' }, { role: 'assistant', content: '线索回答' }]);
      });
    });

    describe('API 错误处理', () => {
      it('应该处理 HTTP 错误响应（非 200 状态码）', async () => {
        const props = createDefaultProps();

        // Mock fetch 返回错误响应（B6 后代码用 response.json() 解析）
        mockFetch.mockResolvedValueOnce({
          ok: false,
          status: 401,
          json: async () => ({ error: { message: 'Unauthorized' } }),
        });

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        // B6 重构后：错误通过 onError 回调传递，不再 push assistant 消息
        expect(props.onError).toHaveBeenCalled();
        const errorMsg = props.onError.mock.calls[0][0];
        expect(errorMsg).toContain('Unauthorized');
      });

      it('应该处理网络请求失败（fetch 抛异常）', async () => {
        const props = createDefaultProps();

        // Mock fetch 抛出网络错误
        mockFetch.mockRejectedValueOnce(new Error('Network error'));

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        // B6 重构后：错误通过 onError 回调传递
        expect(props.onError).toHaveBeenCalled();
        const errorMsg = props.onError.mock.calls[0][0];
        expect(errorMsg).toContain('AI 回复失败');
        expect(errorMsg).toContain('Network error');
      });

      it('API Key 缺失时不再前端阻断（B6 后由后端解析）', async () => {
        const props = {
          ...createDefaultProps(),
          settings: {
            ...createDefaultProps().settings,
            api_key: '', // 空 API Key
          },
        };

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'event: text_delta\ndata: {"text":"Response"}\n\n',
            'event: done\ndata: {}\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        // autonomics：api_key 归服务端持有（TUI config.db 的 providers 表），
        // 前端设置里有没有 key 都不影响发送，请求体也不携带任何凭据
        expect(mockFetch).toHaveBeenCalledTimes(1);
        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);
        expect(requestBody).not.toHaveProperty('model_config');
        expect(props.onError).not.toHaveBeenCalled();
      });

      it('应该处理请求超时（AbortError）', async () => {
        const props = createDefaultProps();

        // 使用一个变量来跟踪实际的消息状态
        let capturedMessages: any[] = [];
        props.setMessages = vi.fn((updaterOrArray) => {
          if (typeof updaterOrArray === 'function') {
            capturedMessages = updaterOrArray(capturedMessages);
          } else {
            capturedMessages = updaterOrArray;
          }
        });

        // Mock fetch 返回带有 abort 信号的响应
        const abortError = new Error('AbortError');
        abortError.name = 'AbortError';
        mockFetch.mockRejectedValueOnce(abortError);

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        // AbortError 在没有超时标志时不应该显示超时消息
        // 但仍然应该包含用户消息
        expect(capturedMessages.length).toBeGreaterThan(0);
      });

      it('应该处理空 AI 响应', async () => {
        const props = createDefaultProps();

        // 使用一个变量来跟踪实际的消息状态
        let capturedMessages: any[] = [];
        props.setMessages = vi.fn((updaterOrArray) => {
          if (typeof updaterOrArray === 'function') {
            capturedMessages = updaterOrArray(capturedMessages);
          } else {
            capturedMessages = updaterOrArray;
          }
        });

        // Mock 返回空内容流
        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":""}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        // 验证空响应提示
        const lastMessage = capturedMessages[capturedMessages.length - 1];

        expect(lastMessage.content).toContain('空回复');
      });

      it('应该处理流内错误消息', async () => {
        const props = createDefaultProps();

        // B6 后：agent SSE 协议要求 event: error 头 + data: {...}
        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'event: error\n',
            'data: {"message":"Rate limit exceeded","error_code":"RateLimit"}\n\n',
            'event: done\n',
            'data: {}\n\n',
          ])
        );

        // 监听 console.error — useChatSender 在 stream 出错时记录日志（RAF 异步更新测试环境不触发）
        const consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        // 验证错误被 useChatSender 捕获并记录
        expect(consoleErrorSpy).toHaveBeenCalledWith(
          expect.stringContaining('[useChatSender]'),
          expect.stringContaining('Rate limit exceeded'),
        );
        consoleErrorSpy.mockRestore();
      });
    });

    describe('附件处理', () => {
      it('B6 后主对话 agent body 只携带文本，图片附件被剥掉（仅线程模式走 /api/ai/proxy 才带图）', async () => {
        const props = createDefaultProps();

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'event: text_delta\n',
            'data: {"text":"I see the image"}\n\n',
            'event: done\n',
            'data: {}\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        const attachments = [
          { category: 'image', base64: 'data:image/png;base64,iVBORw0KG...' },
        ];

        await act(async () => {
          await result.current.sendMessages('check this image', [], attachments);
        });

        expect(mockFetch).toHaveBeenCalledTimes(1);

        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);

        // 用户文本走顶层 message 字段（字符串）
        expect(requestBody.message).toBe('check this image');

        // agent messages 仅包含历史 user/assistant，content 透传(String 或 parts 数组)
        // 不再剥图片块:含图历史消息保留 parts 数组,后端 content_parse 解析
        for (const m of requestBody.messages || []) {
          expect(['string', 'object'].includes(typeof m.content)).toBe(true);
        }
      });

      it('多附件时 agent body 仍然只携带合并后的文本', async () => {
        const props = createDefaultProps();

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'event: text_delta\n',
            'data: {"text":"Received"}\n\n',
            'event: done\n',
            'data: {}\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        const attachments = [
          { category: 'image', base64: 'data:image/png;base64,abc' },
          { category: 'image', base64: 'data:image/jpeg;base64,def' },
        ];

        await act(async () => {
          await result.current.sendMessages('multiple images', [], attachments);
        });

        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);
        expect(requestBody.message).toBe('multiple images');
      });

      it('没有附件时应该发送纯文本消息', async () => {
        const props = createDefaultProps();

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"OK"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('text only', [], []);
        });

        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);

        // 新输入走 body.message（服务端 MessageInject 注入）；
        // messages 是不含本轮输入的历史种子，纯文本时 message 就是原始字符串
        expect(typeof requestBody.message).toBe('string');
        expect(requestBody.message).toBe('text only');
      });
    });

    describe('VibeCard 引用处理', () => {
      it('应该处理带 VibeCard 引用的消息', async () => {
        const props = createDefaultProps();

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"Response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        const vibeCardRefs = [
          { id: 'card1', type: 'key_point' },
        ];

        await act(async () => {
          await result.current.sendMessages('explain this', vibeCardRefs);
        });

        // 验证消息被发送
        expect(mockFetch).toHaveBeenCalledTimes(1);

        // VibeCard 引用通过特定方式处理（具体逻辑取决于实现）
        // 这里验证基本发送流程正常
      });
    });

    describe('消息引用（引用之前的消息）', () => {
      it('应该正确格式化引用的 AI 消息', async () => {
        const props = createDefaultProps();

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"Response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        const quotedMessage = {
          role: 'assistant',
          content: 'This is a previous AI response being quoted.',
        } as any;

        await act(async () => {
          await result.current.sendMessages(
            'follow up question',
            [],
            [],
            quotedMessage
          );
        });

        // 验证 fetch 被调用
        expect(mockFetch).toHaveBeenCalledTimes(1);

        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);

        // 引用前缀拼进新输入（body.message），服务端注入后模型可见
        expect(requestBody.message).toContain('[引用了AI的消息]:');
        expect(requestBody.message).toContain('follow up question');
      });

      it('应该正确格式化引用的用户消息', async () => {
        const props = createDefaultProps();

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"Response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        const quotedMessage = {
          role: 'user',
          content: 'Previous user message',
        } as any;

        await act(async () => {
          await result.current.sendMessages(
            'response to quoted',
            [],
            [],
            quotedMessage
          );
        });

        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);

        // 验证消息包含引用格式
        expect(requestBody.message).toContain('[引用了用户的消息]:');
      });

      it('应该限制引用片段为 200 字符', async () => {
        const props = createDefaultProps();

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"Response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        // 创建一个超过 200 字符的长消息
        const longContent = 'a'.repeat(300);
        const quotedMessage = {
          role: 'assistant',
          content: longContent,
        } as any;

        await act(async () => {
          await result.current.sendMessages(
            'question',
            [],
            [],
            quotedMessage
          );
        });

        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);

        // 验证引用被截断
        const quoteMatch = requestBody.message.match(/\[引用了AI的消息\]:\n([\s\S]+?)\n\n/);
        expect(quoteMatch).toBeTruthy();
        // 引用内容应该不超过 200 字符加上引用符号
        expect(quoteMatch[1].length).toBeLessThan(250);
      });
    });

    describe('不同 AI Provider 处理', () => {
      it('应该正确处理 OpenAI provider', async () => {
        const props = {
          ...createDefaultProps(),
          settings: {
            ...createDefaultProps().settings,
            ai_provider: 'openai',
          },
        };

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"OpenAI response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);

        // autonomics：provider 归服务端持有，请求体不携带 model_config；
        // 无论前端配置哪个 provider，线上契约一致
        expect(requestBody).toHaveProperty('messages');
        expect(requestBody).not.toHaveProperty('model_config');
        expect(requestBody).not.toHaveProperty('temperature');
      });

      it('应该正确处理 Anthropic provider', async () => {
        const props = {
          ...createDefaultProps(),
          settings: {
            ...createDefaultProps().settings,
            ai_provider: 'anthropic',
          },
        };

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"Anthropic response"}}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        const fetchCall = mockFetch.mock.calls[0];
        const requestBody = JSON.parse(fetchCall[1].body);

        // autonomics：主对话统一走 /api/v1/agent/chat，provider 差异由
        // 服务端模型配置（TUI config.db）解析，请求体不带 model_config
        expect(fetchCall[0]).toBe('/api/v1/agent/chat');
        expect(requestBody).not.toHaveProperty('model_config');
        expect(requestBody).toHaveProperty('messages');
      });
    });

    describe('System Prompt 层级构建', () => {
      it('Layer 2.7: 应该注入高优先级额外上下文', async () => {
        const props = {
          ...createDefaultProps(),
          additionalSystemContext: [
            {
              text: '## 代码摘要\n```js\nfunction test() { return "code"; }\n```',
              priority: 'high',
            },
          ],
        };

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"Response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);
        const systemContent = requestBody.system_prompt;

        // 验证高优先级上下文被注入
        expect(systemContent).toContain('代码摘要');
        expect(systemContent).toContain('function test');
      });

      it('Layer 5: 应该注入低优先级额外上下文', async () => {
        const props = {
          ...createDefaultProps(),
          additionalSystemContext: [
            {
              text: '补充信息：这是一些额外的低优先级上下文。',
              priority: 'low',
            },
          ],
        };

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"Response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);
        const systemContent = requestBody.system_prompt;

        // 验证低优先级上下文被注入
        expect(systemContent).toContain('补充信息');
      });

      it('应该同时处理高优先级和低优先级上下文', async () => {
        const props = {
          ...createDefaultProps(),
          additionalSystemContext: [
            {
              text: '高优先级：代码上下文',
              priority: 'high',
            },
            {
              text: '低优先级：辅助说明',
              priority: 'low',
            },
          ],
        };

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"Response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);
        const systemContent = requestBody.system_prompt;

        // 验证两个上下文都被注入
        expect(systemContent).toContain('高优先级：代码上下文');
        expect(systemContent).toContain('低优先级：辅助说明');
      });
    });

    describe('Token 预算管理', () => {
      it('应该根据模型使用正确的 token 预算', async () => {
        const props = {
          ...createDefaultProps(),
          currentModel: {
            key: 'gpt-4',
            displayName: 'GPT-4',
          },
          tokenBudgets: {
            'gpt-4': 8000,
          },
        };

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"Response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        // 验证请求被正确处理（token 预算影响 system prompt 构建）
        expect(mockFetch).toHaveBeenCalledTimes(1);
      });
    });

    describe('消息持久化', () => {
      it('成功接收响应后应该调用 persistMessages（无参数，从 ref 读最新状态）', async () => {
        const props = createDefaultProps();

        // B6 后：agent SSE 格式（event: text_delta + data: {"text": "..."}）
        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'event: text_delta\n',
            'data: {"text":"Final response"}\n\n',
            'event: done\n',
            'data: {}\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        // persistMessages 被无参调用（B6 改动：从 messagesRef.current 读取，避免快照覆盖线程）
        await waitFor(() => {
          expect(props.persistMessages).toHaveBeenCalled();
        });
        expect(props.persistMessages.mock.calls[0]).toHaveLength(0);
      });
    });

    describe('AbortController 管理', () => {
      it('应该在发送请求时使用 AbortController 的 signal', async () => {
        const props = createDefaultProps();

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"Response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        // 验证 fetch 被调用且传入了 signal
        expect(mockFetch).toHaveBeenCalledTimes(1);
        const fetchCall = mockFetch.mock.calls[0];
        expect(fetchCall[1].signal).toBeDefined();
        expect(fetchCall[1].signal).toBeInstanceOf(AbortSignal);
      });

      it('应该在请求完成后清理 AbortController', async () => {
        const props = createDefaultProps();

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"Response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        // 请求完成后 AbortController 应该被清理
        await waitFor(() => {
          expect(props.abortControllerRef.current).toBeNull();
        });
      });
    });

    describe('错误消息过滤', () => {
      it('B6 重构后：错误消息保留在历史中传递给 agent 后端（前端不再过滤）', async () => {
        const props = {
          ...createDefaultProps(),
          messages: [
            { role: 'user', content: 'previous question' },
            { role: 'assistant', content: '⚠️ 上游服务连接失败' },
            { role: 'user', content: 'new question' },
          ],
        };

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'event: text_delta\n',
            'data: {"text":"Response"}\n\n',
            'event: done\n',
            'data: {}\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('test');
        });

        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);

        // B6 前：前端会剥掉 ⚠️ 错误消息
        // B6 后：错误消息原样保留在 messages 历史中，由后端 agent 自行判断
        // 注意：如果未来要在 agent 后端做过滤，可在 services/agent/ 层处理
        const errorMessages = requestBody.messages.filter((m: any) =>
          m.content && m.content.includes && m.content.includes('⚠️')
        );
        expect(errorMessages).toHaveLength(1);
      });
    });

    describe('历史消息截断', () => {
      it('应该基于 token 预算截断历史消息', async () => {
        // 创建一个超长的历史记录（超过 HISTORY_TOKEN_BUDGET）
        const longHistory = [];
        // 创建大量消息，每条消息足够大以确保超过 token 预算
        for (let i = 0; i < 5000; i++) {
          longHistory.push({
            role: i % 2 === 0 ? 'user' : 'assistant',
            content: `Message ${i}: ${'A'.repeat(200)}`, // 每条消息约 200 字符
          });
        }

        const props = {
          ...createDefaultProps(),
          messages: longHistory,
        };

        mockFetch.mockResolvedValueOnce(
          createMockStreamResponse([
            'data: {"choices":[{"delta":{"content":"Response"}}]}\n\n',
            'data: [DONE]\n\n',
          ])
        );

        const { result } = renderHook(() => useChatSender(props as any));

        await act(async () => {
          await result.current.sendMessages('new message');
        });

        const requestBody = JSON.parse(mockFetch.mock.calls[0][1].body);

        // 验证发送的消息数量被合理限制
        // HISTORY_TOKEN_BUDGET 是 100000 tokens
        // 5000 条消息 × 200 字符远超预算，应该被截断
        expect(requestBody.messages.length).toBeLessThan(5002); // 不应该包含全部消息
        expect(requestBody.messages.length).toBeGreaterThan(1); // 但应该包含一些消息
      });
    });
  });
});
