/**
 * 翻译相关 API 客户端
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * jayread 的翻译能力（标题/摘要翻译、PDF 段落预翻译、悬停/双击术语流式翻译）
 * 全部依赖后端 LLM 翻译管线与 MinerU 段落坐标。autonomics 两者皆无，函数桩化。
 *
 * 降级策略：
 * - 查询类（getTranslationStatus / getParagraphTranslationStatus）返回中性
 *   状态值，UI 保持「未翻译」且不发起任何请求
 * - 触发类（translatePaper / triggerParagraphTranslation）reject，调用方 toast
 * - 流式（translateTextStream）直接回调 onError，悬停/双击翻译提示失败
 * - 批量读取（getParagraphTranslations）返回空集合，翻译缓存为空即无高亮层
 *
 * 导出签名与 jayread 版本一致；UI 入口隐藏属 Phase 5 清扫。
 */
import type {
  TranslateTextResponse,
  TranslationStatus,
} from '@/types';

/** 单条 ParagraphTranslation（对应 jayread ParagraphTranslationDto） */
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
  /** 段落边界框（PDF points，原点左上，Y 向下）。 */
  bboxX0?: number;
  bboxY0?: number;
  bboxX1?: number;
  bboxY1?: number;
}

export interface ParagraphTranslationsResponse {
  translations: ParagraphTranslationDto[];
  total: number;
}

export interface TriggerParagraphTranslationResponse {
  status: string;
  itemId: string;
}

export interface ParagraphTranslationStatusResponse {
  status: string;
  progress: { translated: number; total: number; failed: number };
}

interface TranslateTextOptions {
  source_lang?: string;
  target_lang?: string;
  signal?: AbortSignal;
}

export interface SSECallbacks {
  onToken?: (token: string) => void;
  onDone?: (data: { translation: string }) => void;
  onError?: (error: string) => void;
}

/**
 * 按需翻译任意文本
 *
 * ⚠️ 桩：autonomics 无翻译管线。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * @param {string} text - 待翻译文本
 * @param {TranslateTextOptions} [options={}] - 可选配置
 * @returns {Promise<TranslateTextResponse>} 翻译结果（本实现恒 reject）
 */
export async function translateText(text: string, _options: TranslateTextOptions = {}): Promise<TranslateTextResponse> {
  void _options;
  throw new Error(`翻译暂不可用（autonomics 无翻译管线），原文：${text.slice(0, 40)}`);
}

/**
 * 获取论文的翻译状态
 *
 * ⚠️ 桩：恒返回 idle / 无翻译内容，UI 保持未翻译态。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * @param {string} paperId - 论文 ID
 * @returns {Promise<TranslationStatus>} 翻译状态对象
 */
export async function getTranslationStatus(_paperId: string): Promise<TranslationStatus> {
  return { status: 'idle', title_zh: null, abstract_zh: null, error: null };
}

/**
 * 触发论文标题/摘要翻译
 *
 * ⚠️ 桩：autonomics 无翻译管线。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * @param {string} paperId - 论文 ID
 * @returns {Promise<TranslationStatus>} 翻译任务状态对象
 */
export async function translatePaper(_paperId: string): Promise<TranslationStatus> {
  throw new Error('翻译暂不可用（autonomics 无翻译管线）');
}

/**
 * 获取指定页面的段落预翻译结果
 *
 * ⚠️ 桩：无段落翻译数据，恒返回空集合。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * @param {string} paperId - 论文 ID
 * @param {number} pageIdx - 页码索引（0-based）
 */
export async function getParagraphTranslations(_paperId: string, _pageIdx: number): Promise<ParagraphTranslationsResponse> {
  return { translations: [], total: 0 };
}

/**
 * 触发段落预翻译（后台异步任务）
 *
 * ⚠️ 桩：autonomics 无翻译管线。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * @param {string} paperId - 论文 ID
 * @param {boolean} [force=false] - 是否强制重新翻译
 */
export async function triggerParagraphTranslation(_paperId: string, _force: boolean = false): Promise<TriggerParagraphTranslationResponse> {
  throw new Error('段落预翻译暂不可用（autonomics 无翻译管线）');
}

/**
 * 获取段落预翻译进度
 *
 * ⚠️ 桩：无任务，恒返回 idle 空进度。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * @param {string} paperId - 论文 ID
 */
export async function getParagraphTranslationStatus(_paperId: string): Promise<ParagraphTranslationStatusResponse> {
  return { status: 'idle', progress: { translated: 0, total: 0, failed: 0 } };
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
 * 流式文本翻译 — 用于双击术语翻译
 *
 * ⚠️ 桩：autonomics 无翻译管线，直接回调 onError。保持 `{abort}` 返回形状，
 * 调用方（useDoubleClickTerm）的生命周期代码无需感知差异。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 *
 * @param {string} text - 待翻译文本
 * @param {SSECallbacks} callbacks - 回调函数
 * @returns {{ abort: () => void }} 包含 abort 方法的控制对象
 */
export function translateTextStream(
  text: string,
  { onError }: SSECallbacks,
): { abort: () => void } {
  void text;
  // 异步回调，保持与真实实现一致的「调用后立刻返回控制器」语义
  setTimeout(() => {
    onError?.('翻译暂不可用（autonomics 无翻译管线）');
  }, 0);

  return { abort: () => { /* 无流可中断 */ } };
}
