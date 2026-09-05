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
 * - 解析管线：PDF 走 MinerU 异步解析（上传即返回 pending → SSE 推
 *   progress/done/error），reparse / stream 已接通；translate / download 仍桩化，
 *   UI 走既有的失败分支优雅降级；期刊指标（fetchMetadata → fetch-metrics）已接通
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
  journalMetricsIndex,
  journalKeyOfPaper,
  type BibArticle,
  type BibArticleListResponse,
  type BibArticleDetail,
  type BibCollectionLike,
  type BibFullText,
  type BibJournalMetrics,
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
 * POST /articles/upload（multipart）：服务端快速本地抽取只用于扫描
 * DOI / arXiv 标识符（命中则经 gateway 拉真实元数据，离线落按标识符索引
 * 的存根；未命中建 `local:{uuid}` 手工条目），可选挂进分类。**PDF 正文
 * 由 MinerU 异步解析**：返回即 pending 终态，进度走 getStreamUrl 的
 * SSE 流，done 后全文可读（txt/html 仍为同步抽取，返回即 done）。
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

  // PDF 上传本身即时返回（正文异步解析），但 txt/html 仍走服务端同步
  // 抽取，扫描件场景已不存在——保留 5 分钟上限作为大文件传输的兜底
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
  // 期刊指标（IF / 分区）同理：/journals/metrics 是期刊级缓存表，一次拉全量
  // 在前端按规范化期刊名匹配注入。解析状态（MinerU pending/processing/done/
  // failed）同理：/fulltext-statuses 一次拉全量按 article_id 倒排覆盖。
  const [res, collections, journals, parseStatuses] = await Promise.all([
    request<BibArticleListResponse>(`/articles?${query}`, { signal }),
    request<{ collections?: BibCollectionLike[] }>('/collections', { signal }).catch(() => null),
    request<{ journals?: BibJournalMetrics[] }>('/journals/metrics', { signal }).catch(() => null),
    getFulltextStatuses(signal),
  ]);

  const membership = collectionMembershipOf(collections?.collections);
  const metrics = journalMetricsIndex(journals?.journals);
  const parseIndex = new Map(parseStatuses.map((row) => [row.article_id, row]));
  const papers = articlesOf(res).map((article) => {
    const parse = parseIndex.get(article.id);
    const paper = {
      ...articleToPaper(article, undefined, undefined, parse),
      category_ids: membership.get(article.id) ?? [],
    };
    // 期刊指标注入：匹配不上（无期刊名 / 未缓存）时保持 articleToPaper
    // 的 null 默认值，列渲染为「-」。
    const key = journalKeyOfPaper(paper);
    const journal = key ? metrics.get(key) : undefined;
    if (journal) {
      return {
        ...paper,
        impact_factor: journal.impact_factor ?? null,
        impact_factor_5: journal.impact_factor_5 ?? null,
        jcr_quartile: journal.jcr_quartile ?? null,
        ssci_quartile: journal.ssci_quartile ?? null,
        cas_quartile: journal.cas_quartile ?? null,
        cas_quartile_base: journal.cas_quartile_base ?? null,
        cas_small: journal.cas_small ?? null,
        cas_top: journal.cas_top ?? false,
        cas_warning: journal.cas_warning ?? null,
      };
    }
    return paper;
  });

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
 * 期刊指标（IF / 分区）与列表页同源：并行拉一次 /journals/metrics 按期刊名
 * 匹配注入（client 有缓存，重复打开不会重复请求）。
 *
 * @param {string} id - 论文 ID（safeId）
 * @param {AbortSignal} [signal] - 可选的 AbortSignal
 * @returns {Promise<GetPaperResponse>} 论文详细信息
 */
export async function getPaper(id: string, signal?: AbortSignal): Promise<GetPaperResponse> {
  const realId = fromSafeId(id);
  const [detail, journals] = await Promise.all([
    request<BibArticleDetail>(
      `/articles/${encodeIdSegment(realId)}?offset=0&limit=1`,
      { signal },
    ),
    request<{ journals?: BibJournalMetrics[] }>('/journals/metrics', { signal }).catch(() => null),
  ]);

  // jayread 的 Paper 类型没有 attachments 字段（PaperReaderPage 用 as any 读），
  // 这里以属性展开的方式挂上，避免改类型定义。
  const paper = detailToPaper(detail) as unknown as GetPaperResponse;
  const key = journalKeyOfPaper(paper as Paper);
  const journal = key ? journalMetricsIndex(journals?.journals).get(key) : undefined;
  if (journal) {
    return {
      ...(paper as object),
      impact_factor: journal.impact_factor ?? null,
      impact_factor_5: journal.impact_factor_5 ?? null,
      jcr_quartile: journal.jcr_quartile ?? null,
      ssci_quartile: journal.ssci_quartile ?? null,
      cas_quartile: journal.cas_quartile ?? null,
      cas_quartile_base: journal.cas_quartile_base ?? null,
      cas_small: journal.cas_small ?? null,
      cas_top: journal.cas_top ?? false,
      cas_warning: journal.cas_warning ?? null,
    } as GetPaperResponse;
  }
  return paper;
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
// 解析管线（MinerU）
// ============================================================

/** /fulltext-statuses 的行形状 */
interface BibFulltextParseStatus {
  article_id: string;
  parse_status: string;
  parse_engine: string | null;
  parse_error: string | null;
}

/**
 * 拉全量解析状态（article_id 索引）
 *
 * GET /fulltext-statuses → {statuses: [...]}。列表页与 getPapers 并行拉取后
 * 在前端倒排（循 /collections + /journals/metrics 的既有模式）。
 * 失败按空表处理：解析状态只是展示增强，不应拖垮列表。
 */
export async function getFulltextStatuses(signal?: AbortSignal): Promise<BibFulltextParseStatus[]> {
  const res = await request<{ statuses?: BibFulltextParseStatus[] }>('/fulltext-statuses', { signal })
    .catch(() => ({ statuses: [] as BibFulltextParseStatus[] }));
  return res.statuses ?? [];
}

/**
 * 重新解析论文
 *
 * POST /articles/{enc}/reparse：把 fulltexts 行拨回 pending 并重新拉起
 * MinerU 后台任务。409 = 已有解析在跑；400 = 无 VFS 源文件或未配 token。
 * 进度经 getStreamUrl 的 SSE 流推送，调用方应在成功后订阅。
 */
export async function reparsePaper(id: string, _engine: string = 'mineru', signal?: AbortSignal): Promise<ReparsePaperResponse> {
  void _engine; // 后端固定走 MinerU（vlm），engine 参数仅为签名兼容
  const realId = fromSafeId(id);
  const res = await request<{ id?: string; message?: string }>(
    `/articles/${encodeIdSegment(realId)}/reparse`,
    { method: 'POST', signal },
  );
  invalidateCache('/fulltext-statuses');
  return { message: res?.message ?? '重新解析已开始' };
}

/**
 * 获取论文解析进度的 SSE 端点 URL
 *
 * 同源相对路径（同 jayread）。事件：progress{percent,stage} / done{parse_engine,
 * markdown_length} / error{message}，另有 10s 心跳 ping。鉴权由 useSSEProgress
 * 的 fetch 读取器带 Authorization 头（EventSource 设不了自定义头）。
 */
export function getStreamUrl(id: string): string {
  return `/api/v1/bib/articles/${encodeIdSegment(fromSafeId(id))}/parse/stream`;
}

/**
 * 手动触发期刊元数据获取（影响因子 / JCR / 中科院分区）
 *
 * autonomics: POST /articles/{id}/fetch-metrics —— 按该文章的期刊名查
 * EasyScholar 并写入 journal_metrics 缓存（期刊级缓存，同一期刊只查一次）。
 * 阻塞式端点：返回时结果已落库，调用方刷新列表即可见。
 * 未命中（无 key / 期刊不在库）不报错，metrics 为 null → `fetched: false`。
 */
export async function fetchMetadata(id: string, signal?: AbortSignal): Promise<FetchMetadataResponse> {
  const realId = fromSafeId(id);
  const data = await request<{ journal?: string | null; metrics?: unknown }>(
    `/articles/${encodeIdSegment(realId)}/fetch-metrics`,
    { method: 'POST', signal },
  );
  return {
    message: data?.journal ?? '',
    journal: data?.journal ?? null,
    fetched: !!data?.metrics,
  };
}

// ============================================================
// 桩：autonomics 不提供的能力
// ============================================================

/**
 * 检查是否有论文正在等待元数据获取
 *
 * ⚠️ 桩：恒返回空列表（jayread 时代的轮询消费者已随死代码清扫移除）。
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
