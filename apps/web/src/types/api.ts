/**
 * JayRead API Request/Response Types
 */

import type {
  Paper, Category, Conversation, Message, Highlight, Attachment, Settings,
  GuideTree, GuideTreeNode, BlocksData, Block, ImportResult,
  ResolveDuplicatesResult, WebSearchResponse, TavilyKeyCheck,
  PaperTypesRegistry, TranslationStatus, TranslateTextResponse, TranslateParagraphResponse,
  FileParseResult, MetadataPendingCheck, ConversationSummary, GuideNodeSearchResult,
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
  id: number;
  parse_status: string;
  title: string;
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

// Guide API
export type GetGuideTreeResponse = GuideTree;

export interface GenerateGuideV2Request {
  strategy?: 'adaptive' | 'full' | 'section_only';
  focus_sections?: string[];
}

export interface GenerateGuideV2Response {
  task_id: string;
  status: 'queued' | 'processing';
  estimated_time?: number;
}

export interface CancelGuideResponse {
  message: string;
  task_id: string;
}

export type GetBlocksResponse = BlocksData;

export interface GetNodeDetailResponse {
  id: string;
  title: string;
  summary: string;
  page_idx: number;
  bboxes: number[][];
  importance: number;
  children: GuideTreeNode[];
}

export interface RegenerateNodeResponse {
  task_id: string;
  status: string;
}

export interface UpdateNodeImportanceResponse {
  node: GuideTreeNode;
}

export interface SearchGuideNodesRequest {
  query: string;
  search_type?: 'keyword' | 'semantic';
  limit?: number;
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
  paper_id: number;
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

// Search API
export interface WebSearchRequest {
  query: string;
}

export type WebSearchAPIResponse = WebSearchResponse;
export type CheckTavilyKeyResponse = TavilyKeyCheck;

// Files API
export type ParseFileResponse = FileParseResult;

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
