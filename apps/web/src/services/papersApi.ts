/**
 * 论文相关 API 客户端（autonomics）
 *
 * 后端契约：`/api/v1/bib/articles`（见 crates/tui-http/src/bib.rs）。
 *
 * 与 jayread 后端的差异全部在本层消化：
 * - ID：组件拿到的是 safeId（base64url），发请求前还原成真实 ID 并 encodeURIComponent
 * - 分页/排序：jayread 用 page/pageSize + camelCase 排序键，autonomics 用
 *   offset/limit + created_at|updated_at|title|year 白名单；后端不支持的排序键
 *   在前端本地排序补齐
 * - 无解析管线：reparse / stream / metadata / translate / download 全部桩化，
 *   UI 走既有的失败分支优雅降级
 */
import request, { invalidateCache } from './client';
import {
  fromSafeId,
  encodeIdSegment,
  detailToPaper,
  articleToPaper,
  paperToArticle,
  articlesOf,
  collectionMembershipOf,
  type BibArticle,
  type BibArticleListResponse,
  type BibArticleDetail,
  type BibCollectionLike,
  type BibFullText,
} from './mapping';
import type {
  UploadPaperResponse,
  GetPapersParams,
  GetPapersResponse,
  GetPaperResponse,
  GetMarkdownResponse,
  ReparsePaperResponse,
  DeletePaperResponse,
  FetchMetadataResponse,
  MetadataPendingCheck,
  GetPaperTypesResponse,
  TranslatePaperResponse,
  TranslationStatus,
  DownloadPdfResponse,
  Paper,
} from '@/types';

// ============================================================
// 排序键映射
// ============================================================

/**
 * autonomics `/articles` 真正支持排序的列。
 * 后端用 Rust 枚举白名单映射列名，禁止字符串拼 SQL，所以这里是封闭集合。
 */
const BACKEND_SORT_FIELDS = new Set(['created_at', 'updated_at', 'title', 'year']);

/**
 * jayread camelCase 排序键 → autonomics 列名。
 *
 * usePaperStore.sortField 的完整取值集合是
 *   createdAt | updatedAt | publicationYear | citationCount | impactFactor |
 *   impactFactor5 | jcrQuartile | markdownLength | paperType
 * （见 paperListConstants.tsx 的 SORT_FIELD_MAP）。autonomics 没有引用数 /
 * 影响因子 / JCR 分区数据，这些列只能在前端排。
 */
const SORT_FIELD_MAP: Record<string, string> = {
  createdAt: 'created_at',
  updatedAt: 'updated_at',
  publicationYear: 'year',
  year: 'year',
  title: 'title',
};

/** 取 Paper 的本地排序值；null/undefined 排在最后（与 SQL NULLS LAST 语义一致）。 */
function localSortValue(paper: Paper, sortField: string): string | number {
  switch (sortField) {
    case 'citationCount': return paper.citation_count ?? 0;
    case 'impactFactor': return paper.impact_factor ?? 0;
    case 'impactFactor5': return paper.impact_factor_5 ?? 0;
    case 'jcrQuartile': return paper.jcr_quartile ?? '';
    case 'markdownLength': return paper.markdown_length ?? 0;
    case 'paperType': return paper.paper_type ?? '';
    default: return paper.created_at ?? '';
  }
}

/**
 * 把 jayread 的列表参数映射成 autonomics 的 query string。
 * 返回值同时带出「是否需要前端本地排序」，供 getPapers 决定是否补一次排序。
 */
function buildListQuery(params: GetPapersParams): {
  query: string;
  localSortField: string | null;
} {
  const sp = new URLSearchParams();

  // 搜索词：jayread 叫 search，autonomics 叫 query
  if (params.search) sp.set('query', params.search);

  // 分页：jayread page(1-based)/pageSize → autonomics offset/limit。
  // 后端 /articles 的 limit 上限是 2000（其他端点 500），列表页一次拉全量。
  const pageSize = params.pageSize ?? 1000;
  const page = params.page ?? 1;
  sp.set('limit', String(pageSize));
  sp.set('offset', String((page - 1) * pageSize));

  // 排序：后端支持的透传，不支持的记下来由前端排（但仍要给后端一个合法 sort，
  // 否则 400 —— 这里固定用 created_at 作为稳定底序）。
  const requestedSort = params.sort ?? 'createdAt';
  const mappedSort = SORT_FIELD_MAP[requestedSort];
  let localSortField: string | null = null;
  if (mappedSort && BACKEND_SORT_FIELDS.has(mappedSort)) {
    sp.set('sort', mappedSort);
  } else {
    sp.set('sort', 'created_at');
    localSortField = requestedSort;
  }
  sp.set('order', params.order ?? 'desc');

  // 分类筛选：jayread categoryId（collection id）→ autonomics collection_id
  if (params.categoryId) sp.set('collection_id', params.categoryId);
  // 「未分类」筛选
  if (params.uncategorized) sp.set('unfiled', 'true');

  return { query: sp.toString(), localSortField };
}

/** 在前端按 jayread 语义补排一次（后端不支持的排序键）。 */
function sortPapersLocally(papers: Paper[], sortField: string, order: 'asc' | 'desc'): Paper[] {
  const dir = order === 'asc' ? 1 : -1;
  return [...papers].sort((a, b) => {
    const va = localSortValue(a, sortField);
    const vb = localSortValue(b, sortField);
    if (va === vb) return 0;
    if (typeof va === 'number' && typeof vb === 'number') return (va - vb) * dir;
    return String(va).localeCompare(String(vb)) * dir;
  });
}

// ============================================================
// CRUD
// ============================================================

/** POST /articles/upload 的响应（服务端同步完成抽取 + 建档 + 归类） */
interface UploadArticleResponse {
  created: boolean;
  article: BibArticle;
  fulltext: BibFullText | null;
  text_chars: number | null;
}

/**
 * 上传 PDF 文件创建论文记录
 *
 * POST /articles/upload（multipart）：服务端抽取文本、扫描 DOI / arXiv
 * 标识符、命中则经 gateway 拉真实元数据（离线落按标识符索引的存根）、
 * 未命中建 `local:{uuid}` 手工条目，并可选挂进分类——一步到位，**没有
 * 后续解析流水线**，返回即终态。
 *
 * 返回完整 Paper（含 id/parse_status/title 等），列表行可直接渲染；
 * `created: false` 表示标识符命中已有文献、文件挂到了原条目上（调用方
 * 据此更新而非重复插入）。
 *
 * @param {File} file - PDF 文件对象
 * @param {string|null} [categoryId=null] - 可选的分类 ID
 * @param {AbortSignal} [signal] - 可选的 AbortSignal，用于取消请求
 * @returns {Promise<UploadPaperResponse>} 上传结果（`id` 为 safeId）
 */
export async function uploadPaper(
  file: File,
  categoryId: string | null = null,
  signal?: AbortSignal,
): Promise<UploadPaperResponse> {
  const form = new FormData();
  form.append('file', file, file.name);
  if (categoryId !== null && categoryId !== undefined && categoryId !== '') {
    form.append('category_id', String(categoryId));
  }

  // 文本抽取（含 OCR 回退）在服务端同步完成，扫描件可能要跑 tesseract——
  // 默认 30s 超时对大 PDF 太紧，放宽到 5 分钟
  const res = await request<UploadArticleResponse>('/articles/upload', {
    method: 'POST',
    body: form,
    timeout: 300000,
    signal,
  });

  const paper = articleToPaper(res.article, res.fulltext);
  return {
    ...paper,
    created: res.created,
    ...(categoryId ? { category_ids: [String(categoryId)] } : {}),
  } as UploadPaperResponse;
}

/**
 * 获取论文列表
 *
 * GET /articles?query&offset&limit&sort&order&collection_id&unfiled
 *
 * @param {Object} [params={}] - 查询参数（jayread 形状）
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<GetPapersResponse>} 论文列表（`papers[].id` 为 safeId）
 */
export async function getPapers(params: GetPapersParams = {}, signal?: AbortSignal): Promise<GetPapersResponse> {
  const { query, localSortField } = buildListQuery(params);

  // Article 不携带集合成员关系，而 jayread 的列表行直接读 paper.category_ids
  // （PaperListPage 用它建「论文 → 分类」映射）。并行多拉一次 /collections 在
  // 前端倒排，比按文章逐个查成员便宜一个数量级，且命中 client 的各自缓存。
  const [res, collections] = await Promise.all([
    request<BibArticleListResponse>(`/articles?${query}`, { signal }),
    request<{ collections?: BibCollectionLike[] }>('/collections', { signal }).catch(() => null),
  ]);

  const membership = collectionMembershipOf(collections?.collections);
  const papers = articlesOf(res).map((article) => ({
    ...articleToPaper(article),
    category_ids: membership.get(article.id) ?? [],
  }));

  return {
    papers: localSortField ? sortPapersLocally(papers, localSortField, params.order ?? 'desc') : papers,
    total: typeof res?.total === 'number' ? res.total : papers.length,
  };
}

/**
 * 获取论文详情
 *
 * GET /articles/{enc} → {article, fulltext, fulltext_pagination, annotations}。
 * 全文元信息在信封**顶层**（Article 结构体没有任何全文字段），必须显式交给
 * detailToPaper，否则「有 PDF 的文献」会被误判成 bib_import、打开元数据视图。
 *
 * 全文文本切片只要 1 个字符：这里只需要 file_path / file_size / total_chars
 * 这些元数据，不把 10 万字符的正文拖回来（正文由 getMarkdown 按需分页拉）。
 *
 * 同时把合成的附件数组挂在返回对象上：PaperReaderPage 会读
 * `(paper as any).attachments` 来判断是否显示「正在查看非主附件」横幅。
 *
 * @param {string} id - 论文 ID（safeId）
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<GetPaperResponse>} 论文详细信息
 */
export async function getPaper(id: string, signal?: AbortSignal): Promise<GetPaperResponse> {
  const realId = fromSafeId(id);
  const detail = await request<BibArticleDetail>(
    `/articles/${encodeIdSegment(realId)}?offset=0&limit=1`,
    { signal },
  );

  // jayread 的 Paper 类型没有 attachments 字段（PaperReaderPage 用 as any 读），
  // 这里以属性展开的方式挂上，避免改类型定义。
  return detailToPaper(detail) as unknown as GetPaperResponse;
}

/**
 * PATCH 风格更新论文
 *
 * 只传需要改的字段。autonomics 的 `PUT /articles/{enc}` 收完整 Article JSON，
 * 所以这里先 GET 当前值再合并，对 jayread 调用方保持「部分更新」观感。
 *
 * 目前唯一的线上调用是 `{ hoverTranslationEnabled }` 切换 —— autonomics 没有
 * 段落翻译能力，这个字段无对应列，合并结果与原值相同，此时跳过 PUT 直接返回，
 * 避免一次无意义写。
 *
 * @param {string} id - 论文 ID
 * @param {Partial<Pick<Paper, 'hoverTranslationEnabled'>>} patch - 待更新字段
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<Paper>} 更新后的论文对象
 */
export async function updatePaper(
  id: string,
  patch: Partial<Pick<Paper, 'hoverTranslationEnabled'>>,
  signal?: AbortSignal,
): Promise<Paper> {
  const realId = fromSafeId(id);
  const current = await getPaper(id, signal);

  // 计算合并后的 Article；hoverTranslationEnabled 无后端对应物，不会产生差异
  const nextArticle = paperToArticle(current as Paper, realId);
  const before = JSON.stringify(nextArticle);
  const after = JSON.stringify(paperToArticle({ ...(current as Paper), ...patch } as Paper, realId));

  if (before !== after) {
    // 后端收完整 Article JSON：先用路径 id 覆盖 article.id、恢复 created_at，
    // 再整体 upsert。响应是 {"article": ...}。
    await request<{ article?: BibArticle }>(`/articles/${encodeIdSegment(realId)}`, {
      method: 'PUT',
      body: JSON.parse(after),
      signal,
    });
    invalidateCache(`/articles/${encodeIdSegment(realId)}`);
    invalidateCache('/articles');
  }

  return { ...(current as Paper), ...patch } as Paper;
}

/**
 * 删除论文
 *
 * @param {string} id - 论文 ID
 * @returns {Promise<DeletePaperResponse>} 删除确认消息
 */
export async function deletePaper(id: string): Promise<DeletePaperResponse> {
  const realId = fromSafeId(id);
  // 后端返回 200 + {"deleted":bool,"id","rows_removed"}（id 不存在也是 200，
  // deleted:false），不是 204 —— 成败只看状态码，body 不需要解
  await request<{ deleted?: boolean }>(`/articles/${encodeIdSegment(realId)}`, { method: 'DELETE' });
  invalidateCache('/articles');
  invalidateCache('/collections');
  return { message: '已删除' };
}

// ============================================================
// 全文
// ============================================================

/**
 * 获取论文 PDF 的下载/预览 URL
 *
 * 返回相对路径字符串（同源部署），调用方可能再拼 base 前缀。
 *
 * @param {string} id - 论文 ID
 * @returns {string} PDF 文件的 API URL
 */
export function getPdfUrl(id: string): string {
  return `/api/v1/bib/articles/${encodeIdSegment(fromSafeId(id))}/fulltext/raw`;
}

/**
 * 获取论文解析后的全文内容
 *
 * autonomics 的 `/fulltext` 按**字符**分页：`?offset=&limit=`（limit 上限
 * 500_000，默认 100_000）。响应是
 * `{fulltext: {text_content}, pagination: {next_offset}}`，`next_offset` 为
 * null 表示到末尾 —— 没有 has_more / next_page 这类布尔标志。
 *
 * 这里循环取完再拼接。每页按后端默认 100_000 字符取，避免一篇长文拆成几十个
 * 小请求。
 *
 * @param {string} id - 论文 ID
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<GetMarkdownResponse>} 全文内容；无全文时 markdown 为空串
 */
export async function getMarkdown(id: string, signal?: AbortSignal): Promise<GetMarkdownResponse> {
  const realId = fromSafeId(id);
  const enc = encodeIdSegment(realId);
  const PAGE_CHARS = 100_000; // 与后端 DEFAULT_FULLTEXT_CHARS 对齐

  const readPage = async (offset: number): Promise<{
    content: string;
    nextOffset: number | null;
  }> => {
    const data = await request<{ fulltext?: { text_content?: string | null }; pagination?: { next_offset?: number | null } }>(
      `/articles/${enc}/fulltext?offset=${offset}&limit=${PAGE_CHARS}`,
      { signal },
    );
    const text = typeof data?.fulltext?.text_content === 'string' ? data.fulltext.text_content : '';
    const next = data?.pagination?.next_offset;
    return { content: text, nextOffset: typeof next === 'number' ? next : null };
  };

  const MAX_PAGES = 10000; // 兜底：防止后端分页语义异常导致死循环
  const parts: string[] = [];
  let offset = 0;
  for (let i = 0; i < MAX_PAGES; i++) {
    const { content, nextOffset } = await readPage(offset);
    parts.push(content);
    // next_offset 为 null（或不再前进）即到末尾
    if (nextOffset === null || nextOffset <= offset) break;
    offset = nextOffset;
  }

  const markdown = parts.join('');
  return { markdown, parse_status: markdown ? 'done' : 'none' };
}

// ============================================================
// 桩：autonomics 不提供的能力
// ============================================================

/**
 * 重新解析论文
 *
 * ⚠️ 桩：autonomics 无 PDF 解析管线（无 MinerU/OCR 后端任务）。
 * 保持签名不变，调用方（handleReparse）会 toast 失败提示。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function reparsePaper(_id: string, _engine: string = 'mineru', signal?: AbortSignal): Promise<ReparsePaperResponse> {
  void _id; void _engine; void signal;
  throw new Error('重新解析暂不可用（autonomics 无解析管线）');
}

/**
 * 获取论文 SSE 实时状态推送端点 URL
 *
 * ⚠️ 桩：autonomics 无解析进度流。返回一个永不产生事件的 URL，
 * EventSource 会自行静默重试，进度条只是不更新（jayread 的既有降级行为）。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export function getStreamUrl(id: string): string {
  void id;
  return `/api/v1/bib/no-parse-stream`;
}

/**
 * 手动触发期刊元数据获取
 *
 * ⚠️ 桩：autonomics 不做期刊抓取（影响因子 / JCR 分区等后端无数据源）。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function fetchMetadata(_id: string, signal?: AbortSignal): Promise<FetchMetadataResponse> {
  void _id; void signal;
  throw new Error('获取期刊信息暂不可用（autonomics 无元数据抓取管线）');
}

/**
 * 检查是否有论文正在等待元数据获取
 *
 * ⚠️ 桩：恒返回空列表 → useMetadataPolling 立即停止轮询并刷新列表。
 * 这是「无事可做」的正确答案，不会产生任何网络请求。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function checkMetadataPending(_signal?: AbortSignal): Promise<MetadataPendingCheck> {
  return { pending_ids: [] };
}

/**
 * 获取系统支持的文献类型列表
 *
 * autonomics 的类型即 BibTeX entry_type，映射到 jayread 的 paper_type 后返回。
 * 当前无线上调用方（仅单测），保留以对齐签名。
 */
export async function getPaperTypes(): Promise<GetPaperTypesResponse> {
  // 与 mapping.ts 的 ENTRY_TYPE_MAP 保持同一取值域；UI 只在筛选下拉里用 label
  return {
    types: {
      article: { id: 'article', name: '期刊论文', name_en: 'Journal Article', icon: 'FileTextOutlined', color: 'geekblue' },
      review: { id: 'review', name: '综述', name_en: 'Review', icon: 'FileTextOutlined', color: 'purple' },
      book: { id: 'book', name: '书籍', name_en: 'Book', icon: 'BookOutlined', color: 'orange' },
      conference_paper: { id: 'conference_paper', name: '会议论文', name_en: 'Conference Paper', icon: 'AudioOutlined', color: 'cyan' },
      thesis: { id: 'thesis', name: '学位论文', name_en: 'Thesis', icon: 'ReadOutlined', color: 'green' },
      preprint: { id: 'preprint', name: '预印本', name_en: 'Preprint', icon: 'RocketOutlined', color: 'volcano' },
    },
  };
}

/**
 * 翻译论文标题和摘要
 *
 * ⚠️ 桩：autonomics 无翻译管线。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function translatePaper(_id: string, signal?: AbortSignal): Promise<TranslatePaperResponse> {
  void _id; void signal;
  throw new Error('翻译暂不可用（autonomics 无翻译管线）');
}

/**
 * 查询论文翻译状态
 *
 * ⚠️ 桩：恒返回 idle / 无翻译内容，UI 按钮保持未翻译态。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function getTranslationStatus(_id: string, _signal?: AbortSignal): Promise<TranslationStatus> {
  return { status: 'idle', title_zh: null, abstract_zh: null, error: null };
}

/**
 * 通过 DOI 自动下载开放获取 PDF
 *
 * ⚠️ 桩：autonomics 无 Unpaywall 下载管线。
 *
 * stubbed: capability not present in autonomics backend (see plan Phase 5)
 */
export async function downloadPdf(_paperId: string, signal?: AbortSignal): Promise<DownloadPdfResponse> {
  void _paperId; void signal;
  throw new Error('自动下载 PDF 暂不可用（autonomics 无开放获取下载管线）');
}
