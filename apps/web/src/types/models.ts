/**
 * JayRead Core Data Model Types
 */

export interface Paper {
  id: string;
  title: string;
  authors: string[];
  journal_name: string | null;
  publication_year: number | null;
  doi: string | null;
  abstract: string | null;
  parse_status: string;
  title_zh: string | null;
  abstract_zh: string | null;
  category_ids: string[];
  citation_count: number | null;
  impact_factor: number | null;
  paper_type: string | null;
  translation_status: string | null;
  created_at: string;
  updated_at: string;
  filename: string | null;
  parse_engine: string | null;
  parse_error: string | null;
  storage_key: string | null;
  subtitle: string | null;
  type?: string | null;
  extra_metadata?: Record<string, unknown>;
  item_source: string | null;
  source_file: string | null;
  full_summary: string | null;
  summary_status: string | null;
  summary_error: string | null;
  journal_issn: string | null;
  openalex_id: string | null;
  open_access_status: string | null;
  isbn: string | null;
  pmid: string | null;
  url: string | null;
  publisher: string | null;
  source: string | null;
  impact_factor_5: number | null;
  jcr_quartile: string | null;
  ssci_quartile: string | null;
  cas_quartile: string | null;
  cas_quartile_base: string | null;
  cas_small: string | null;
  cas_top: boolean;
  cas_warning: string | null;
  markdown_length: number | null;
  paragraphTranslationStatus?: string | null;
  hoverTranslationEnabled?: boolean;
}

export interface Category {
  id: string;
  name: string;
  parent_id: string | null;
  sort_order: number;
  paper_count: number;
  created_at: string;
  updated_at: string;
  children?: Category[];
}

export interface Conversation {
  id: string;
  title: string;
  paper_id: string;
  is_active: boolean;
  message_count: number;
  created_at: string;
  updated_at: string;
}

export interface Message {
  id: string;
  conversation_id: string;
  role: 'user' | 'assistant' | 'system';
  content: string;
  context_quote: string | null;
  context_pages?: number[];
  created_at: string;
}

export interface HighlightRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Highlight {
  id: string;
  paper_id: string;
  page: number;
  text: string;
  color: string;
  rects: HighlightRect[];
  created_at: string;
}

export interface Attachment {
  id: string;
  paper_id: string;
  filename: string;
  file_type: string;
  file_path: string;
  file_size: number;
  is_primary: boolean;
  created_at: string;
}

export interface Settings {
  model_provider?: string;
  api_key?: string;
  api_base?: string;
  model?: string;
  temperature?: number;
  max_tokens?: number;
  parse_engine?: string;
}

export interface TokenBudgets {
  budgets?: Record<string, number>;
  default_budget?: number;
}

export interface Summary {
  id: string;
  paper_id: string;
  content: string | null;
  status: string;
  error: string | null;
  created_at: string;
}

export interface ImportResult {
  imported: number;
  duplicates: number;
  failed: number;
  duplicate_details: DuplicateDetail[];
  imported_paper_ids: string[];
}

/** 后端 BibRecord 的 serde 序列化形态（导入去重链路里原样往返） */
export interface BibRecordData {
  entry_type: string;
  key: string;
  title: string;
  authors: string[];
  doi?: string | null;
  year?: number | null;
  abstract?: string | null;
  source?: string | null;
  publisher?: string | null;
  url?: string | null;
  journal?: string | null;
  volume?: string | null;
  number?: string | null;
  pages?: string | null;
}

export interface DuplicateDetail {
  /** 批次内唯一标识（key#index），resolve 时原样回传 */
  id: string;
  key: string;
  /** 新记录标题 */
  title: string;
  reason: string;
  existing_item_id: string;
  existing_title: string;
  record: BibRecordData;
}

export type ResolutionAction = 'skip' | 'replace' | 'keep_both';

export interface DuplicateResolution {
  id: string;
  action: ResolutionAction;
  /** replace 必填 */
  existing_item_id?: string;
  /** replace / keep_both 必填，由 import 响应原样回传 */
  record?: BibRecordData;
}

export interface ResolveDuplicatesResult {
  created: number;
  skipped: number;
  updated: number;
  paper_ids: string[];
}

export interface PaperType {
  id: string;
  name: string;
  name_en: string;
  icon: string;
  color: string;
}

export type PaperTypesRegistry = Record<string, PaperType>;

export interface TranslationStatus {
  // 'failed' 来自后端 paragraph_translation_status（pretranslate catch 时写入），
  // 与 'error'（HTTP/解析错误）语义区分但都视为终止态
  status: 'idle' | 'generating' | 'done' | 'error' | 'failed';
  title_zh: string | null;
  abstract_zh: string | null;
  abstract_sentences?: AbstractSentence[];
  error: string | null;
}

export interface AbstractSentence {
  original: string;
  translated: string;
}

export interface TranslateTextResponse {
  translation: string;
  original: string;
}

export interface TranslateParagraphResponse {
  translation: string;
  original: string;
  sentences: SentencePair[];
}

export interface SentencePair {
  original: string;
  translated: string;
}

export interface MetadataPendingCheck {
  pending_ids: string[];
}

export interface ConversationSummary {
  summary: string;
  summarized_count: number;
}

export interface SSEProgressEvent {
  phase?: string;
  percent?: number;
  eta_ms?: number;
  message?: string;
}

export interface ApiError {
  detail: string;
}
