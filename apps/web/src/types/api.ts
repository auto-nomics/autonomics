/**
 * Autonomics API Request/Response Types
 */

import type {
  Paper, Category, Conversation, Message, Highlight, Attachment, Settings,
  ImportResult,
  ResolveDuplicatesResult,
  PaperTypesRegistry, TranslationStatus, TranslateTextResponse, TranslateParagraphResponse,
  MetadataPendingCheck, ConversationSummary,
  TokenBudgets,
} from './models';

// Papers API
export interface GetPapersParams {
  sort?: string;
  order?: 'asc' | 'desc';
  paper_type?: string;
  search?: string;
  // 分页（默认 page=1, pageSize=20，上限 1000）。PaperListPage 一次拉 1000 用虚拟列表渲染。
  // 字段名按后端 papers.rs:69 的 #[serde(rename = "pageSize")] 用 camelCase。
  page?: number;
  pageSize?: number;
  // 分类筛选
  categoryId?: string;
  uncategorized?: boolean;
  // 其他过滤（同样按后端 serde rename 用 camelCase）
  isFavorite?: boolean;
  isRead?: boolean;
  tagId?: string;
}

export interface GetPapersResponse {
  papers: Paper[];
  total: number;
}

export type GetPaperResponse = Paper;

export interface GetMarkdownResponse {
  markdown: string;
  parse_status: string;
}

export interface UploadPaperResponse {
  // autonomics 文章 ID 是 `doi:10.1000/x` 形态的字符串，service 出口转成 base64url
  // safeId（见 services/mapping.ts）。jayread 时代的自增数字主键已不存在。
  id: string;
  parse_status: string;
  title: string;
  /** 服务端建档结果：false = 标识符命中已有文献，文件挂到了原条目（勿重复插入列表） */
  created?: boolean;
  /** service 实际返回完整 Paper（展开字段，列表行直接可用） */
  [key: string]: unknown;
}

export interface ReparsePaperResponse {
  message: string;
}

export interface DeletePaperResponse {
  message: string;
}

export interface FetchMetadataResponse {
  message: string;
}

export interface TranslatePaperResponse {
  status: 'idle' | 'generating' | 'done' | 'error';
  title_zh: string | null;
  abstract_zh: string | null;
  error: string | null;
}

export interface GetPaperTypesResponse {
  types: PaperTypesRegistry;
}

// Chat API
export interface GetMessagesResponse {
  messages: Message[];
}

export interface SaveMessagesRequest {
  messages: Message[];
}

export interface SaveMessagesResponse {
  message: string;
}

export interface ClearMessagesResponse {
  message: string;
}

export interface GetConversationsResponse {
  conversations: Conversation[];
}

export interface CreateConversationResponse {
  conversation_id: number;
  message: string;
}

export interface ActivateConversationResponse {
  conversation_id: number;
  message: string;
}

export interface DeleteConversationResponse {
  message: string;
}

export interface SummarizeConversationRequest {
  messages: Message[];
  paper_title: string;
}

// Settings API
export interface GetSettingsResponse {
  settings: Settings;
}

export type UpdateSettingsRequest = Partial<Settings>;

export interface UpdateSettingsResponse {
  message: string;
}

export type GetTokenBudgetsResponse = TokenBudgets;

// Attachments API
export interface GetAttachmentsResponse {
  attachments: Attachment[];
  total: number;
}

export interface UploadAttachmentsResponse {
  attachments: Attachment[];
  total: number;
}

export interface DeleteAttachmentResponse {
  message: string;
}

export interface SetPrimaryAttachmentResponse {
  message: string;
}

// Translation API
export interface TranslateTextRequest {
  text: string;
  source_lang?: string;
  target_lang?: string;
}

export interface TranslateParagraphRequest {
  text: string;
  source_lang?: string;
  target_lang?: string;
}

// Bibliography API
export interface ImportBibliographyParams {
  file: File;
  categoryId?: number | null;
  format?: string | null;
}

export interface AttachPdfParams {
  paperId: number;
  file: File;
}

export interface AttachPdfResponse {
  message: string;
  /** 论文 ID（safeId 字符串；autonomics 无自增数字主键） */
  paper_id: string;
}

export interface DownloadPdfResponse {
  message: string;
  paper_id: number;
  success: boolean;
}

// Highlights API
export interface FetchHighlightsResponse {
  highlights: Highlight[];
  total: number;
}

export interface CreateHighlightRequest {
  page: number;
  text: string;
  rects: Array<{ x: number; y: number; w: number; h: number }>;
  color?: string;
}

export interface UpdateHighlightRequest {
  color?: string;
}

// Categories API
export interface GetCategoryTreeResponse {
  categories: Category[];
  total_count: number;
  uncategorized_count: number;
}

export interface CreateCategoryRequest {
  name: string;
  parent_id?: number | null;
}

export type CreateCategoryResponse = Category;
export type RenameCategoryResponse = Category;

export interface DeleteCategoryResponse {
  message: string;
}

export interface AssignPapersRequest {
  paper_ids: number[];
  category_ids: number[];
}

export interface AssignPapersResponse {
  message: string;
}

export interface UnassignPapersRequest {
  paper_ids: number[];
  category_ids: number[];
}

export interface UnassignPapersResponse {
  message: string;
}

export interface GetPaperCategoriesResponse {
  paper_id: number;
  category_ids: number[];
}

export interface MoveCategoryRequest {
  parent_id: number | null;
  sort_order: number;
}

export type MoveCategoryResponse = Category;

// Common
export interface ApiResponse {
  message: string;
}

export interface RequestOptions {
  method?: 'GET' | 'POST' | 'PUT' | 'DELETE' | 'PATCH';
  body?: Record<string, unknown> | FormData;
  headers?: Record<string, string>;
  timeout?: number;
  retries?: number;
  signal?: AbortSignal;
}
