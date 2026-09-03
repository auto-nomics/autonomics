/**
 * streamParser.js
 * ============================================================
 * SSE 流式响应解析工具模块
 *
 * 文件功能：
 * 从 useChatSender.js 中提取的流式响应解析逻辑，处理 AI API 返回的 SSE 数据流。
 *
 * 核心职责：
 * - 解析 OpenAI 和 Anthropic 两种格式的 SSE 响应
 * - 提取文本增量并累加到 assistantContent
 * - 检测流内错误并返回错误信息
 * - 实时更新消息状态（通过 setMessages 回调）
 * - 处理空回复和 API 错误
 *
 * 设计决策：
 * - 提取为独立模块，降低 useChatSender.js 的复杂度
 * - 纯函数设计，无副作用（除了调用传入的回调）
 * - 支持 setMessages 自定义（主消息/线程消息两种路径）
 * - 错误时显示友好的错误提示
 *
 * @module ai-chat/utils/streamParser
 */

/**
 * 解析 SSE 流式响应
 *
 * 处理 AI API 返回的 SSE (Server-Sent Events) 流式响应，提取文本增量。
 * 支持 OpenAI 和 Anthropic 两种格式。
 *
 * 数据格式说明：
 * - OpenAI: data: {"choices":[{"delta":{"content":"..."}}]}
 * - Anthropic: data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"..."}}
 * - 结束标记: data: [DONE]
 *
 * 错误处理：
 * - 流内错误：parsed.error 字段存在时记录错误
 * - 空回复：assistantContent 为空时显示提示
 * - HTTP 错误：由调用方处理，此函数只处理流数据
 *
 * 竞态条件防护：
 * - 通过 isRequestStillActive 回调检查当前请求是否仍然活跃
 * - 当用户快速切换论文时，旧论文的延迟响应不会覆盖新论文的消息状态
 * - 如果请求已不再活跃，则跳过所有状态更新
 *
 * @typedef {Object} ParseStreamResponseParams
 * @property {Response} response - fetch 返回的 Response 对象
 * @property {'openai' | 'anthropic'} provider - AI 提供商
 * @property {Array<{role: string, content: string, id?: string}>} newMessages - 新的消息列表
 * @property {Function} setMessages - 设置消息列表的函数
 * @property {Function} [isRequestStillActive] - 检查请求是否仍然活跃的回调函数
 *
 * @typedef {Object} ParseStreamResponseResult
 * @property {string} content - AI 回复的完整文本内容
 * @property {string|null} error - 错误信息（成功时为 null）
 *
 * @param {ParseStreamResponseParams} params - 参数对象
 * @returns {Promise<ParseStreamResponseResult>} 解析结果
 */

// Agent SSE 事件类型
export const AGENT_EVENT = {
  TEXT_DELTA: 'text_delta',
  TOOL_CALL_START: 'tool_call_start',
  TOOL_CALL_RESULT: 'tool_call_result',
  DONE: 'done',
  CONVERSATION_CREATED: 'conversation_created',
  ERROR: 'error',
} as const;

// Agent 流错误码
export const AGENT_ERROR = {
  CANCELLED: 'CANCELLED',
  EMPTY_RESPONSE: 'EMPTY_RESPONSE',
} as const;

export async function parseStreamResponse({
  response,
  provider,
  newMessages,
  setMessages,
  isRequestStillActive,
  requestId,
}: {
  response: Response;
  provider: 'openai' | 'anthropic';
  newMessages: Array<Record<string, any>>;
  setMessages: React.Dispatch<React.SetStateAction<any[]>>;
  isRequestStillActive?: () => boolean;
  requestId?: string;
}): Promise<{ content: string; error: string | null }> {
  // 获取流式响应的 reader
  const reader = response.body!.getReader();

  // 文本解码器（将 Uint8Array 转为字符串）
  const decoder = new TextDecoder();

  // 累加的 AI 回复内容
  let assistantContent = '';

  // 流式错误（流内错误，非 HTTP 错误）
  let streamError: string | null = null;

  // 原始响应文本（用于错误诊断）
  let rawResponseText = '';

  // 行缓冲区 - 用于跨chunk的不完整行
  let lineBuffer = '';

  // requestAnimationFrame 批处理状态
  let rafId: number | null = null;
  let pendingUpdate = false;
  // Ref to track latest assistantContent for RAF callback (stale closure fix)
  let contentRef: { current: string } = { current: assistantContent };

  // 先添加一个空的 assistant 消息（用于流式更新占位）
  // 检查请求是否仍然活跃，防止旧响应覆盖新状态
  const assistantId = `msg-${Date.now()}-${Math.random().toString(36).slice(2, 11)}`;
  if (!isRequestStillActive || isRequestStillActive()) {
    const msgsWithAssistant = [...newMessages, { role: 'assistant', content: '', id: assistantId }];
    setMessages(msgsWithAssistant);
  }

  // ========== 循环读取流数据 ==========

  /**
   * 流式读取循环
   *
   * 读取流程：
   * 1. reader.read() 返回 { done, value }
   * 2. done=true 表示流结束，退出循环
   * 3. value 是 Uint8Array，解码为字符串
   * 4. 解析 SSE 格式的数据行
   * 5. 提取文本增量并累加
   * 6. 实时更新消息状态
   *
   * 竞态条件防护：每次迭代检查请求是否仍然活跃
   */
  while (true) {
    // 检查请求是否仍然活跃，如果用户已切换论文则立即退出
    // 防止继续处理已过期的流式响应
    if (isRequestStillActive && !isRequestStillActive()) {
      // 请求已过期，静默退出不更新状态
      return { content: '', error: AGENT_ERROR.CANCELLED };
    }

    // 读取下一个数据块
    const { done, value } = await reader.read();

    // Check if request is still active after await (race condition fix)
    // A new request could have started during the await, so verify we're still processing the same request
    if (isRequestStillActive && !isRequestStillActive()) {
      // Request is no longer active, silently exit without updating state
      return { content: '', error: AGENT_ERROR.CANCELLED };
    }

    // 流结束，退出循环
    if (done) break;

    // 解码数据块为字符串
    const chunk = decoder.decode(value, { stream: true });
    rawResponseText += chunk;

    // 将chunk追加到缓冲区并按\n分割
    lineBuffer += chunk;
    const lines = lineBuffer.split('\n');

    // 保留最后一个可能不完整的行到缓冲区
    // 如果chunk以\n结尾，最后一个元素是空字符串，不需要保留
    const lastLine = lines[lines.length - 1];
    if (lastLine !== '') {
      lineBuffer = lastLine;
      lines.pop();
    } else {
      lineBuffer = '';
      lines.pop(); // 移除末尾空字符串
    }

    // 过滤出 SSE 数据行（以 "data: " 开头的行）
    const dataLines = lines.filter((l) => l.startsWith('data:'));

    // ========== 解析每行数据 ==========

    for (const line of dataLines) {
      // 移除 "data: " 前缀
      const data = line.replace(/^data:\s?/, '');

      // 结束标记，跳过
      if (data === '[DONE]') break;

      try {
        // 解析 JSON 数据
        const parsed = JSON.parse(data);

        // ========== 流内错误检测 ==========

        /**
         * 检测流内错误
         *
         * 某些 API 会在流中返回错误（而非 HTTP 错误）
         * 格式：data: {"error": {"message": "..."}}
         */
        if (parsed.error) {
          const errMsg = parsed.error.message || parsed.error.msg || parsed.error;
          streamError = typeof errMsg === 'string' ? errMsg : JSON.stringify(errMsg);
          continue; // 跳过此数据块，继续读取后续数据
        }

        // ========== 提取文本增量 ==========

        if (provider === 'anthropic') {
          // Anthropic 格式：data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"..."}}
          if (parsed.type === 'content_block_delta' && parsed.delta?.type === 'text_delta') {
            assistantContent += parsed.delta.text || '';
            contentRef.current = assistantContent;
          }
        } else {
          // OpenAI 格式：data: {"choices":[{"delta":{"content":"..."}}]}
          const delta = parsed.choices?.[0]?.delta?.content || '';
          assistantContent += delta;
          contentRef.current = assistantContent;
        }
      } catch (e) {
        // JSON 解析失败，静默忽略（可能是未完成的数据块）
        if (Math.random() < 0.01) {
          // Rate-limit to ~1% of failures
          console.warn('[streamParser] Failed to parse SSE JSON:', line);
        }
      }

      // ========== 实时更新消息 ==========

      /**
       * 实时更新消息状态（使用 requestAnimationFrame 批处理）
       *
       * 每次收到文本增量后，批量更新 messages 状态
       * 实现"打字机效果"的视觉反馈，同时避免高频重渲染
       *
       * 竞态条件防护：在更新状态前检查请求是否仍然活跃
       * 如果用户已切换到其他论文，则跳过更新，防止旧响应覆盖新状态
       */
      if (!isRequestStillActive || isRequestStillActive()) {
        if (!pendingUpdate) {
          pendingUpdate = true;
          rafId = requestAnimationFrame(() => {
            setMessages((prev) => {
              const updated = [...prev];
              // 更新最后一条消息（assistant 消息）- use ref to avoid stale closure
              updated[updated.length - 1] = {
                role: 'assistant',
                content: contentRef.current,
                id: assistantId,
              };
              return updated;
            });
            pendingUpdate = false;
          });
        }
      }
    }
  }

  // ========== 流结束后检查 ==========
  // Flush any pending requestAnimationFrame update
  if (rafId !== null) {
    cancelAnimationFrame(rafId);
    rafId = null;
  }
  if (pendingUpdate || !assistantContent.trim()) {
    // Ensure the final content is rendered
    if (!isRequestStillActive || isRequestStillActive()) {
      setMessages((prev) => {
        const updated = [...prev];
        updated[updated.length - 1] = {
          role: 'assistant',
          content: assistantContent,
          id: assistantId,
        };
        return updated;
      });
    }
  }

  /**
   * 检查空回复
   *
   * 如果 AI 没有返回任何内容（assistantContent 为空）：
   * 1. 检查是否有流内错误（streamError）
   * 2. 尝试从原始响应中解析错误信息
   * 3. 显示友好的错误提示
   */
  if (!assistantContent.trim()) {
    // 没有内容，检查是否有错误
    if (!streamError && rawResponseText.trim()) {
      // 尝试从原始响应中解析错误
      try {
        const rawJson = JSON.parse(rawResponseText.trim());
        // 尝试多个可能的错误字段
        streamError =
          rawJson.msg ||
          rawJson.error?.message ||
          rawJson.error ||
          rawJson.message ||
          rawJson.detail ||
          null;
        // 确保错误信息是字符串
        if (streamError && typeof streamError !== 'string') {
          streamError = JSON.stringify(streamError);
        }
      } catch {
        // JSON 解析失败，静默忽略
      }
    }

    if (streamError) {
      // 有流内错误，显示错误信息
      // 检查请求是否仍然活跃，防止旧响应覆盖新状态
      if (!isRequestStillActive || isRequestStillActive()) {
        setMessages((prev) => {
          const updated = [...prev];
          updated[updated.length - 1] = {
            role: 'assistant',
            content: `⚠️ API 错误: ${streamError}`,
          };
          return updated;
        });
      }
      return { content: '', error: streamError };
    } else {
      // 无错误但内容为空，显示默认提示
      // 检查请求是否仍然活跃，防止旧响应覆盖新状态
      if (!isRequestStillActive || isRequestStillActive()) {
        setMessages((prev) => {
          const updated = [...prev];
          updated[updated.length - 1] = {
            role: 'assistant',
            content: '⚠️ AI 返回了空回复。请检查模型名称是否正确，或尝试更换模型。',
          };
          return updated;
        });
      }
      return { content: '', error: 'EMPTY_RESPONSE' };
    }
  }

  // ========== 返回成功结果 ==========

  return { content: assistantContent, error: null };
}

export default parseStreamResponse;

/**
 * 把数组里所有还在 'running' 的工具卡翻成 'abandoned'，并写一句兜底 result 文案。
 * 用于 DONE 事件 / 流结束时兜底，避免 UI 一直显示"执行中"撒谎。
 * @returns 是否真的翻过（用于决定要不要 updateMessage 重渲染）
 */
function sweepAbandonedToolCalls(
  toolCalls: Array<{ status: string; result: unknown }>
): boolean {
  let swept = false;
  for (const tc of toolCalls) {
    if (tc.status === 'running') {
      tc.status = 'abandoned';
      if (!tc.result) {
        tc.result = '工具未返回结果（流已结束或中断）';
      }
      swept = true;
    }
  }
  return swept;
}

/**
 * Parse agent SSE stream
 *
 * Reads SSE events from the response stream.
 * Events: text_delta, tool_call_start, tool_call_result, done, error
 *
 * @typedef {Object} ParseAgentStreamParams
 * @property {Response} response - fetch 返回的 Response 对象
 * @property {Array<{role: string, content: string, id?: string}>} newMessages - 新的消息列表
 * @property {Function} setMessages - 设置消息列表的函数
 * @property {Function} [isRequestStillActive] - 检查请求是否仍然活跃的回调函数
 *
 * @typedef {Object} ParseAgentStreamResult
 * @property {string} content - AI 回复的完整文本内容
 * @property {string|null} error - 错误信息（成功时为 null）
 * @property {string|null} conversationId - 对话 ID
 *
 * @param {ParseAgentStreamParams} params - 参数对象
 * @returns {Promise<ParseAgentStreamResult>} 解析结果
 */
export async function parseAgentStream({
  response,
  newMessages,
  setMessages,
  isRequestStillActive,
  requestId,
  onActivity,
}: {
  response: Response;
  newMessages: Array<Record<string, any>>;
  setMessages: React.Dispatch<React.SetStateAction<any[]>>;
  isRequestStillActive?: () => boolean;
  requestId?: string;
  /**
   * 每个 SSE chunk 到达时调用 —— 给调用方做 idle timer reset 用。
   *
   * 为什么需要:旧 useChatSender 用 setTimeout(STREAM_TIMEOUT_MS=120000) 给整个 fetch
   * 设总时长上限,长任务(如批量处理)可能跑几十分钟,总时长 timeout 必中。
   * 改用 idle timeout 模式后,只要 SSE 持续有 event 流入(后端 service.rs 每 10s 发 ping),
   * 永不超时;只在 N 秒完全无活动时才 abort(应对真 hang)。onActivity 在每次 chunk 到达
   * 时回调,调用方据此 reset idle timer。
   */
  onActivity?: () => void;
}): Promise<{ content: string; error: string | null; conversationId: string | null }> {
  const reader = response.body!.getReader();
  const decoder = new TextDecoder();

  let assistantContent = '';
  const toolCalls: Array<{
    id: string;
    name: string;
    status: string;
    input: unknown;
    result: string | null;
  }> = [];
  let streamError: string | null = null;
  let conversationId: string | null = null;
  let lineBuffer = '';
  let currentEvent = '';
  let currentDataLines: string[] = []; // Collect multiple data: lines for the same event

  // Generate unique ID for this assistant message
  const assistantId = `msg-${Date.now()}-${Math.random().toString(36).slice(2, 11)}`;

  // requestAnimationFrame 批处理状态
  let rafId: number | null = null;
  let pendingContent = assistantContent;
  let pendingToolCalls = [...toolCalls];
  let pendingUpdate = false;

  // Placeholder assistant message
  const updateMessage = (content: string, tc?: typeof toolCalls) => {
    pendingContent = content;
    pendingToolCalls = tc || [...toolCalls];

    if (!pendingUpdate) {
      pendingUpdate = true;
      rafId = requestAnimationFrame(() => {
        if (!isRequestStillActive || isRequestStillActive()) {
          setMessages((prev) => {
            const updated = [...prev];
            updated[updated.length - 1] = {
              role: 'assistant',
              content: pendingContent,
              id: assistantId,
              toolCalls: pendingToolCalls,
            };
            return updated;
          });
        }
        pendingUpdate = false;
      });
    }
  };

  if (!isRequestStillActive || isRequestStillActive()) {
    setMessages([
      ...newMessages,
      { role: 'assistant', content: '', id: assistantId, toolCalls: [] },
    ]);
  }

  // Parse incrementally as chunks arrive (streaming UX)
  try {
    while (true) {
      if (isRequestStillActive && !isRequestStillActive()) {
        return { content: '', error: AGENT_ERROR.CANCELLED, conversationId: null };
      }

      let value;
      let done;
      try {
        ({ done, value } = await reader.read());
      } catch (err: any) {
        if (err.name === 'AbortError' || isRequestStillActive?.() === false) {
          return { content: '', error: AGENT_ERROR.CANCELLED, conversationId: null };
        }
        return { content: '', error: `Stream read error: ${err.message}`, conversationId: null };
      }

      // 收到 chunk → 通知调用方 reset idle timer。
      // 即使是 done 也算 activity(让调用方清理 timer)。
      onActivity?.();

      // Race condition fix: check if request is still active after await
      // A new request could have started during the await, so verify we're still processing the same request
      if (isRequestStillActive && !isRequestStillActive()) {
        return { content: '', error: AGENT_ERROR.CANCELLED, conversationId: null };
      }

      if (done) break;

      const chunk = decoder.decode(value, { stream: true });

      // Buffer and process complete lines incrementally
      lineBuffer += chunk;
      const lines = lineBuffer.split('\n');

      // Keep last potentially incomplete line in buffer
      const lastLine = lines[lines.length - 1];
      if (lastLine !== '') {
        lineBuffer = lastLine;
        lines.pop();
      } else {
        lineBuffer = '';
        lines.pop(); // Remove trailing empty string
      }

      // Process SSE events as they arrive
      for (const line of lines) {
        const trimmed = line.trim();

        // Blank line marks end of event - process accumulated data
        if (trimmed === '') {
          // Process the accumulated event data
          if (currentEvent && currentDataLines.length > 0) {
            const combinedData = currentDataLines.join('\n');
            try {
              const data = JSON.parse(combinedData);

              switch (currentEvent) {
                case AGENT_EVENT.TEXT_DELTA:
                  assistantContent += data.text || '';
                  updateMessage(assistantContent);
                  break;

                case AGENT_EVENT.TOOL_CALL_START:
                  toolCalls.push({
                    id: data.tool_use_id,
                    name: data.name,
                    status: 'running',
                    input: data.input,
                    result: null,
                  });
                  updateMessage(assistantContent);
                  break;

                case AGENT_EVENT.TOOL_CALL_RESULT: {
                  const tc = toolCalls.find((t) => t.id === data.tool_use_id);
                  if (tc) {
                    tc.status = data.is_error ? 'error' : 'completed';
                    tc.result = data.preview || '';
                  }
                  updateMessage(assistantContent);
                  break;
                }

                case AGENT_EVENT.DONE: {
                  if (data.conversation_id) {
                    conversationId = data.conversation_id;
                  }
                  // Sweep: 迭代结束时仍有 'running' 的工具 = 后端漏发 tool_call_result
                  // （正常路径下 stream.rs 在 DONE 之前已发完所有 result；走到这里说明状态机出 bug）
                  // 翻 'abandoned' 让 UI 别一直显示"执行中"
                  if (sweepAbandonedToolCalls(toolCalls)) {
                    updateMessage(assistantContent);
                  }
                  break;
                }

                case AGENT_EVENT.CONVERSATION_CREATED:
                  conversationId = data.conversation_id;
                  break;

                case AGENT_EVENT.ERROR: {
                  const errMsg = data.message || 'Unknown error';
                  const errorCode = data.error_code || '';

                  let friendlyMsg: string;
                  if (errorCode.includes('RateLimit') || errMsg.includes('429')) {
                    friendlyMsg = `🚫 **LLM 限额超限(429)**\n\n${errMsg}\n\n💡 请等待几分钟后重试,或在设置中切换其他模型。`;
                  } else if (errorCode.includes('AuthExpired') || errMsg.includes('401')) {
                    friendlyMsg = `🔑 **API Key 无效或已过期(401)**\n\n${errMsg}\n\n💡 请检查 API Key 配置。`;
                  } else if (errorCode.includes('Overloaded') || errMsg.includes('529')) {
                    friendlyMsg = `⚡ **LLM 服务过载(529)**\n\n${errMsg}\n\n💡 服务端繁忙,请稍后重试。`;
                  } else {
                    friendlyMsg = `❌ **请求失败** [${errorCode}]\n\n${errMsg}`;
                  }

                  streamError = friendlyMsg;
                  updateMessage(friendlyMsg);
                  break;
                }
              }
            } catch {
              // Skip malformed JSON
            }
          }
          // Reset for next event
          currentEvent = '';
          currentDataLines = [];
          continue;
        }

        if (trimmed.startsWith('event:')) {
          currentEvent = trimmed.slice(6).trim();
        } else if (trimmed.startsWith('data:')) {
          const dataStr = trimmed.slice(5).trim();
          if (dataStr) {
            currentDataLines.push(dataStr);
          }
        }
      }
    }
  } finally {
    // Memory leak fix: always cancel RAF, even if reader.read() throws
    if (rafId !== null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
    // Sweep: 流结束时仍有 'running' 的工具 = 服务端 hang / 流被切断 / reader 抛异常
    // 翻 'abandoned' 让 UI 别撒谎。同样覆盖 reader.read() 抛异常的路径（finally 必跑）
    if (sweepAbandonedToolCalls(toolCalls)) {
      updateMessage(assistantContent);
    }
    // Release the reader lock to prevent resource leak
    try {
      reader.releaseLock();
    } catch {}
  }

  // Flush any pending requestAnimationFrame update
  if (rafId !== null) {
    cancelAnimationFrame(rafId);
    rafId = null;
  }

  if (streamError) {
    return { content: '', error: streamError, conversationId: null };
  }

  if (!assistantContent.trim() && toolCalls.length === 0) {
    updateMessage('⚠️ AI 返回了空回复。');
    // Flush the error message immediately
    if (rafId !== null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
    if (!isRequestStillActive || isRequestStillActive()) {
      setMessages((prev) => {
        const updated = [...prev];
        updated[updated.length - 1] = {
          role: 'assistant',
          content: '⚠️ AI 返回了空回复。',
          id: assistantId,
          toolCalls: [],
        };
        return updated;
      });
    }
    return { content: '', error: AGENT_ERROR.EMPTY_RESPONSE, conversationId: null };
  }

  // Ensure final update is flushed
  if (pendingUpdate || rafId !== null) {
    if (rafId !== null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
    if (!isRequestStillActive || isRequestStillActive()) {
      setMessages((prev) => {
        const updated = [...prev];
        updated[updated.length - 1] = {
          role: 'assistant',
          content: assistantContent,
          id: assistantId,
          toolCalls: [...toolCalls],
        };
        return updated;
      });
    }
  }

  return { content: assistantContent, error: null, conversationId };
}
