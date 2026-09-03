/**
 * JayRead 翻译相关 API 客户端
 *
 * 提供按需翻译功能：
 * - POST: 翻译任意文本（用于 PDF 悬停翻译、选中文本翻译等）
 * - GET: 查询论文标题/摘要翻译状态
 * - POST: 触发论文标题/摘要翻译
 */

import request, { isTauri } from './client';
import type {
  TranslateTextResponse,
  TranslationStatus,
} from '@/types';

let _streamBaseUrl: string | null = null;

async function getStreamBaseUrl(): Promise<string> {
  if (_streamBaseUrl) return _streamBaseUrl;

  if (!isTauri) return '';
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    const url = await invoke<string>('get_sidecar_url');
    _streamBaseUrl = url;
    return url;
  } catch (err) {
    throw new Error(`Failed to get sidecar URL from Tauri: ${err}`);
  }
}

interface TranslateTextOptions {
  source_lang?: string;
  target_lang?: string;
  signal?: AbortSignal;
}

/** 单条 ParagraphTranslation（对应后端 ParagraphTranslationDto） */
export interface ParagraphTranslationDto {
  id: string;
  itemId: string;
  pageIdx: number;
  blockId?: string;
  cacheKey: string;
  originalText: string;
  translatedText: string;
  /** sentences 是 JSON 字符串，需 JSON.parse 得到 SentencePair[] */
  sentences: string;
  model: string;
  /** 段落边界框（PDF points，原点左上，Y 向下；与 PDFium device 坐标同系）。
   *  来自 PdfBlock，后端在返回时按 blockId join 注入。坐标命中（findEntryByBBox）的主键。 */
  bboxX0?: number;
  bboxY0?: number;
  bboxX1?: number;
  bboxY1?: number;
}

interface ParagraphTranslationsResponse {
  translations: ParagraphTranslationDto[];
  total: number;
}

interface TriggerParagraphTranslationResponse {
  status: string;
  itemId: string;
}

interface ParagraphTranslationStatusResponse {
  status: string;
  progress: { translated: number; total: number; failed: number };
}

interface SSECallbacks {
  onToken?: (token: string) => void;
  onDone?: (data: { translation: string }) => void;
  onError?: (error: string) => void;
}

/**
 * 按需翻译任意文本
 *
 * 用于 PDF 悬停翻译和选中文本翻译等场景。
 * 直接返回翻译结果，不存储到数据库。
 *
 * @param {string} text - 待翻译文本
 * @param {TranslateTextOptions} [options={}] - 可选配置
 * @returns {Promise<TranslateTextResponse>} 翻译结果
 */
export async function translateText(text: string, options: TranslateTextOptions = {}): Promise<TranslateTextResponse> {
  const { source_lang = 'auto', target_lang = 'zh', signal } = options;

  return request<TranslateTextResponse>('/translate/text', {
    method: 'POST',
    body: {
      text,
      source_lang,
      target_lang,
    },
    timeout: 10000, // 10 秒超时
    signal,
  });
}

/**
 * 获取论文的翻译状态
 *
 * @param {string} paperId - 论文 ID
 * @returns {Promise<TranslationStatus>} 翻译状态对象
 */
export async function getTranslationStatus(paperId: string): Promise<TranslationStatus> {
  return request<TranslationStatus>(`/papers/${paperId}/translate`);
}

/**
 * 触发论文标题/摘要翻译
 *
 * @param {string} paperId - 论文 ID
 * @returns {Promise<TranslationStatus>} 翻译任务状态对象
 */
export async function translatePaper(paperId: string): Promise<TranslationStatus> {
  return request<TranslationStatus>(`/papers/${paperId}/translate`, {
    method: 'POST',
  });
}

/**
 * 获取指定页面的段落预翻译结果
 *
 * 前端按页加载，返回的 translations 是 {cacheKey: {translation, sentences, original}} 映射，
 * 可直接写入 translationCache Map。
 *
 * @param {string} paperId - 论文 ID
 * @param {number} pageIdx - 页码索引（0-based）
 * @returns {Promise<ParagraphTranslationsResponse>}
 */
export async function getParagraphTranslations(paperId: string, pageIdx: number): Promise<ParagraphTranslationsResponse> {
  return request<ParagraphTranslationsResponse>(`/papers/${paperId}/paragraph-translations?page_idx=${pageIdx}`);
}

/**
 * 触发段落预翻译（后台异步任务）
 *
 * @param {string} paperId - 论文 ID
 * @param {boolean} [force=false] - 是否强制重新翻译
 * @returns {Promise<TriggerParagraphTranslationResponse>}
 */
export async function triggerParagraphTranslation(paperId: string, force: boolean = false): Promise<TriggerParagraphTranslationResponse> {
  const query = force ? '?force=true' : '';
  return request<TriggerParagraphTranslationResponse>(`/papers/${paperId}/paragraph-translations${query}`, {
    method: 'POST',
  });
}

/**
 * 获取段落预翻译进度
 *
 * @param {string} paperId - 论文 ID
 * @returns {Promise<ParagraphTranslationStatusResponse>}
 */
export async function getParagraphTranslationStatus(paperId: string): Promise<ParagraphTranslationStatusResponse> {
  return request<ParagraphTranslationStatusResponse>(`/papers/${paperId}/paragraph-translations/status`);
}

export default {
  translateText,
  getTranslationStatus,
  translatePaper,
  getParagraphTranslations,
  triggerParagraphTranslation,
  getParagraphTranslationStatus,
};

// ========== 流式翻译 API ==========

/**
 * 解析 SSE 事件流
 *
 * 从 ReadableStream 中逐行解析 SSE 事件（event: xxx\ndata: xxx\n\n），
 * 调用对应的回调函数。
 *
 * @param {Response} response - fetch Response 对象
 * @param {SSECallbacks} callbacks - 回调函数集合
 * @param {AbortSignal} [signal] - 用于取消的 AbortSignal
 */
async function parseSSEStream(response: Response, callbacks: SSECallbacks, signal?: AbortSignal): Promise<void> {
  const reader = response.body!.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  let currentEvent = '';

  try {
    while (true) {
      if (signal?.aborted) {
        reader.cancel();
        break;
      }

      const { done, value } = await reader.read();
      if (done) break;

      buffer += decoder.decode(value, { stream: true });

      // 逐行解析 SSE
      const lines = buffer.split('\n');
      buffer = lines.pop()!; // 最后一个不完整的行保留在 buffer 中

      for (const line of lines) {
        if (line.startsWith('event: ')) {
          currentEvent = line.slice(7).trim();
        } else if (line.startsWith('data: ')) {
          const rawData = line.slice(6);

          if (currentEvent === 'token') {
            // token 事件：data 是 JSON 字符串（带引号）
            try {
              const token = JSON.parse(rawData);
              callbacks.onToken?.(token);
            } catch {
              // 解析失败时直接使用原始文本（去除引号）
              callbacks.onToken?.(rawData.replace(/^"|"$/g, ''));
            }
          } else if (currentEvent === 'done') {
            try {
              const data = JSON.parse(rawData);
              callbacks.onDone?.(data);
            } catch {
              callbacks.onDone?.({ translation: rawData });
            }
          } else if (currentEvent === 'error') {
            try {
              const data = JSON.parse(rawData);
              callbacks.onError?.(data.error || '翻译失败');
            } catch {
              callbacks.onError?.(rawData);
            }
          }
          currentEvent = '';
        }
      }
    }

    // Process any remaining data in buffer after stream ends
    if (buffer.trim()) {
      const remaining = buffer.trim();
      if (remaining.startsWith('data: ')) {
        const rawData = remaining.slice(6);
        if (currentEvent === 'done') {
          try {
            const data = JSON.parse(rawData);
            callbacks.onDone?.(data);
          } catch {
            callbacks.onDone?.({ translation: rawData });
          }
        } else if (currentEvent === 'error') {
          try {
            const data = JSON.parse(rawData);
            callbacks.onError?.(data.error || '翻译失败');
          } catch {
            callbacks.onError?.(rawData);
          }
        } else if (currentEvent === 'token') {
          try {
            const token = JSON.parse(rawData);
            callbacks.onToken?.(token);
          } catch {
            callbacks.onToken?.(rawData.replace(/^"|"$/g, ''));
          }
        }
      }
    }
  } finally {
    reader.releaseLock();
  }
}

/**
 * 流式文本翻译 — 用于双击术语翻译
 *
 * 通过 SSE 逐 token 返回翻译结果，首 token 在 ~200ms 内到达。
 *
 * @param {string} text - 待翻译文本
 * @param {SSECallbacks} callbacks - 回调函数
 * @returns {{ abort: () => void }} 包含 abort 方法的控制对象
 */
export function translateTextStream(text: string, { onToken, onDone, onError }: SSECallbacks): { abort: () => void } {
  const controller = new AbortController();

  (async () => {
    const timeoutId = setTimeout(() => controller.abort(), 30000);
    try {
      const baseUrl = await getStreamBaseUrl();
      const response = await fetch(`${baseUrl}/api/translate/text/stream`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ text, source_lang: 'auto', target_lang: 'zh' }),
        signal: controller.signal,
      });

      if (!response.ok) {
        const err = await response.json().catch(() => ({ detail: response.statusText }));
        throw new Error(err.detail || `请求失败: ${response.status}`);
      }

      await parseSSEStream(response, { onToken, onDone, onError }, controller.signal);
    } catch (err: unknown) {
      if (err instanceof Error && err.name !== 'AbortError') {
        onError?.(err.message || '翻译失败');
      }
    } finally {
      clearTimeout(timeoutId);
    }
  })();

  return { abort: () => controller.abort() };
}
