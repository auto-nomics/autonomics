/**
 * autonomics 后端契约 ↔ jayread 前端模型 的映射层
 *
 * 为什么需要这一层？
 * - autonomics 的文章 ID 形如 `doi:10.1000/x`（含 `:` 和 `/`），直接进 react-router
 *   路径、URL query 或 localStorage 都不安全。所有 service 出口统一转成 base64url
 *   的 "safeId" 给组件，发请求前再还原成真实 ID —— 组件 / 路由 / 持久化键零改动。
 * - 后端 Article（crates/bib-types/src/types.rs 的题录模型）与前端 Paper（jayread
 *   产品模型）字段不对齐：identifiers 数组要拆成 doi/pmid/arxiv 标量、结构化的
 *   Author 对象要拍平成显示字符串，jayread 独有的产品字段（影响因子、翻译状态、
 *   解析管线状态……）autonomics 没有对应物，需要填中性默认值。
 *
 * 字段映射决策都写在 articleToPaper / paperToArticle 的逐行注释里。
 */
import type { Attachment, Paper } from '@/types';

// ============================================================
// 后端契约类型（crates/bib-types/src/types.rs 的 serde 输出）
// ============================================================

/**
 * autonomics 的结构化作者。
 *
 * Paper.authors 是 `string[]`，所以在边界上要「拍平 / 拆回」：
 * 序列化用 `fore_name + ' ' + last_name`（自然阅读顺序），反序列化用
 * 「最后一个空格分隔的词是姓」的启发式。见 authorToLabel / labelToAuthor。
 */
export interface BibAuthor {
  last_name: string;
  fore_name?: string | null;
  initials?: string | null;
  affiliation?: string | null;
  orcid?: string | null;
  corresponding?: boolean;
}

/** 标识符 kind：doi / pmid / pmc / embase / arxiv / biorxiv / s2 / openalex / other */
export type IdentifierKind = string;

export interface BibIdentifier {
  kind: IdentifierKind;
  value: string;
}

/**
 * autonomics 的全文对象（`FullText`）。
 *
 * 注意：这是「文件 + 抽取文本」的混合体，`text_content` 是当前分页切片，不是
 * 全文。文件元信息（file_path / file_size）才是合成附件与 paper.storage_key 的
 * 来源。
 */
export interface BibFullText {
  article_id: string;
  /** 文件存储路径（vfs，形如 `/articles/xxx.pdf`） */
  file_path: string;
  /** pdf | html | txt */
  file_format: string;
  /** 当前页的抽取文本切片（配合 pagination.offset/limit） */
  text_content?: string | null;
  /** user_upload | open_access */
  source: string;
  file_hash?: string | null;
  file_size?: number | null;
  uploaded_at?: string | null;
}

/** 全文分页信息（`/articles/{enc}/fulltext?offset=&limit=`） */
export interface BibFullTextPagination {
  offset: number;
  limit: number;
  /** 全文字符总数（SQL LENGTH(text_content)） */
  total_chars: number;
  truncated: boolean;
  /** 数字 → 下一页 offset；null → 已到末尾 */
  next_offset: number | null;
}

/**
 * autonomics 的题录（`Article`）。字段与 Rust 结构体一一对应，**没有**
 * entry_type / publisher / url / tags / collections / has_fulltext：
 * - 类型信息只有 `pub_types`（PubMed PublicationType 字符串数组）
 * - 期刊名在 `journal`，`source` 是「来源渠道」枚举（pubmed/crossref/...）
 * - 全文信息不在 Article 里，见 BibArticleDetail
 */
export interface BibArticle {
  id: string;
  title: string;
  authors: BibAuthor[];
  identifiers: BibIdentifier[];
  abstract_text: string | null;
  year: number | null;
  month: number | null;
  journal: string | null;
  volume: string | null;
  issue: string | null;
  pages: string | null;
  issn: string | null;
  essn: string | null;
  language: string | null;
  pub_types: string[];
  keywords: string[];
  /** pubmed | embase | crossref | arxiv | biorxiv | europepmc | semanticscholar | gwascatalog | openalex | manual | zotero | unknown */
  source: string;
  created_at: string | null;
  updated_at: string | null;
}

/**
 * `GET /articles/{enc}` 的响应信封。
 *
 * 全文元信息在**顶层**，不在 article 内部 —— Article 结构体没有任何全文字段。
 * 列表接口 `GET /articles` 不返回全文信息，所以列表行只能拿到题录。
 */
export interface BibArticleDetail {
  article: BibArticle;
  fulltext: BibFullText | null;
  fulltext_pagination: BibFullTextPagination | null;
  annotations: unknown[];
}

/**
 * `GET /articles` 的响应信封。
 *
 * `articles` 是当前页排好序的完整 Article 数组；`hits` 只是给旧客户端的兼容壳
 * （score 恒为 0.0、snippet 恒等于 title），本层只用 `articles`。
 */
export interface BibArticleListResponse {
  total: number;
  articles?: BibArticle[];
  /** 兼容壳，不读 */
  hits?: unknown;
  offset?: number;
  limit?: number;
}

// ============================================================
// safeId：base64url（无 padding）
// ============================================================

const BASE64URL_PATTERN = /^[A-Za-z0-9_-]+$/;

/** UTF-8 → base64url，去掉 padding。输出只含 [A-Za-z0-9_-]，URL / localStorage 安全。 */
export function encodeSafeId(realId: string): string {
  const bytes = new TextEncoder().encode(realId);
  let binary = '';
  for (let i = 0; i < bytes.length; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** base64url → UTF-8 的纯逆变换（不做容错，供往返校验用）。 */
function decodeStrict(safeId: string): string {
  let b64 = safeId.replace(/-/g, '+').replace(/_/g, '/');
  while (b64.length % 4 !== 0) b64 += '=';
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return new TextDecoder().decode(bytes);
}

/**
 * safeId → 真实 ID。
 *
 * 容错：autonomics 的真实 ID 恒含 `:`（`doi:` / `local:` / `arxiv:` 前缀），
 * 而 `:` 不在 base64url 字母表内。所以「字符串里出现 base64url 之外的字节」
 * 或「解码后无法往返」都说明调用方传进来的已经是真实 ID，原样返回即可。
 * 这让 service 层对 safeId / 真实 ID 两种输入都健壮（例如 localStorage 里
 * 残留的旧值、或调用方绕过映射直接传了原始 ID）。
 */
export function decodeSafeId(safeId: string): string {
  if (!safeId || !BASE64URL_PATTERN.test(safeId)) return safeId;
  try {
    const decoded = decodeStrict(safeId);
    // 往返不一致 → 不是我们编码出来的串，当真实 ID 用
    return encodeSafeId(decoded) === safeId ? decoded : safeId;
  } catch {
    return safeId;
  }
}

/** service 出口：真实 ID → safeId（给组件 / 路由 / localStorage） */
export function toSafeId(realId: string): string {
  return encodeSafeId(realId);
}

/** service 入口：组件持有的 id → 真实 ID（发请求用） */
export function fromSafeId(safeId: string): string {
  return decodeSafeId(safeId);
}

/**
 * 真实 ID → URL 路径段。
 *
 * 后端契约：`/articles/{enc}`、`/collections/{colId}/articles/{articleId}` 都收
 * 「真实 ID + 百分号编码」（axum Path 自动解码）。safeId 只用于前端边界
 * （路由 / localStorage），**不要**把 safeId 发给后端 —— 后端按字面查库，查不到。
 */
export function encodeIdSegment(realId: string): string {
  return encodeURIComponent(realId);
}

// ============================================================
// 作者：结构化 Author ↔ 显示字符串
// ============================================================

/**
 * BibAuthor → Paper.authors 的显示字符串。
 *
 * 用「名 姓」的自然阅读顺序（`fore_name + ' ' + last_name`），与后端
 * `Author::display_name()` 的「姓 名」顺序不同：jayread 的作者列表是给人读的
 * 字符串，倒装会让中文界面里的西文作者看起来像姓、名颠倒。
 */
export function authorToLabel(author: BibAuthor | string): string {
  if (typeof author === 'string') return author;
  const last = (author.last_name ?? '').trim();
  const fore = (author.fore_name ?? '').trim();
  return fore ? `${fore} ${last}` : last;
}

/**
 * Paper.authors 的显示字符串 → BibAuthor（PUT 回写用）。
 *
 * 启发式：最后一个空格分隔的词是姓，其余全是名。对 "van der Berg" 这类多词
 * 姓恰好也正确（姓取最后一个词，其余归名）；单名作者只有一个词，全部归姓。
 * 空串返回 null（跳过，不留空作者行）。
 */
export function labelToAuthor(label: string): BibAuthor | null {
  const trimmed = label.trim();
  if (!trimmed) return null;
  const parts = trimmed.split(/\s+/);
  if (parts.length === 1) {
    return { last_name: parts[0], fore_name: null, initials: null, affiliation: null, orcid: null, corresponding: false };
  }
  return {
    last_name: parts[parts.length - 1],
    fore_name: parts.slice(0, -1).join(' '),
    initials: null,
    affiliation: null,
    orcid: null,
    corresponding: false,
  };
}

// ============================================================
// 标识符提取
// ============================================================

function identifierOf(article: BibArticle, kind: IdentifierKind): string | null {
  const hit = article.identifiers?.find((it) => it.kind === kind && it.value);
  return hit ? hit.value : null;
}

// ============================================================
// pub_types → paper_type
// ============================================================

/**
 * autonomics 没有单值的 entry_type，类型信息只存在于 `pub_types`
 * （PubMed PublicationType 字符串数组，如 ["Journal Article","Review"]）。
 *
 * 这里按「特殊 → 一般」的顺序取第一个命中的规则映射成 jayread 的 paper_type
 * （utils/literatureTypes.ts 的 TYPE_DISPLAY 键）；全部未命中时回落 'article'。
 * 顺序很重要：["Book Chapter"] 含 "book"，必须先匹配 chapter。
 */
const PUB_TYPE_RULES: Array<[RegExp, string]> = [
  [/\b(chapter)\b/i, 'book_section'],
  [/\b(review|meta-?analysis)\b/i, 'review'],
  [/\bpreprint\b/i, 'preprint'],
  [/\b(conference|congress|meeting|proceeding)/i, 'conference_paper'],
  [/\b(thesis|dissertation)\b/i, 'thesis'],
  [/\b(technical report|report)\b/i, 'report'],
  [/\bpatent\b/i, 'patent'],
  [/\bdataset\b/i, 'dataset'],
  [/\bnewspaper\b/i, 'newspaper_article'],
  [/\b(book|monograph)\b/i, 'book'],
  [/\b(journal|article)\b/i, 'article'],
];

/**
 * pub_types → paper_type。空数组 / 全未命中 → 'article'。
 *
 * 规则优先、而不是标签顺序优先：["Journal Article","Review"] 里的两条都命中，
 * 但 Review 是更具体的分类，应该赢。PubMed 的 PublicationType 是「全部成立」
 * 的无序标签集合，顺序没有语义。
 */
export function pubTypesToPaperType(pubTypes: string[] | null | undefined): string {
  const labels = pubTypes ?? [];
  for (const [pattern, paperType] of PUB_TYPE_RULES) {
    if (labels.some((label) => pattern.test(label))) return paperType;
  }
  return 'article';
}

/**
 * paper_type → 规范化的 pub_types（paperToArticle 回写用）。
 *
 * 多标签的原始记录（如 ["Journal Article","Review"]）在 paper_type 里已经收敛成
 * 一个值，这里只能给出单标签的规范形式 —— 这是「结构化类型」与「单值类型」
 * 之间不可避免的损耗。articleToPaper 会把原始数组存进
 * extra_metadata[PRIVATE_PUB_TYPES]，回写时优先取它，让编辑往返不失真。
 */
export function paperTypeToPubTypes(paperType: string | null | undefined): string[] {
  switch (paperType) {
    case 'review': return ['Review'];
    case 'book': return ['Book'];
    case 'book_section': return ['Book Chapter'];
    case 'conference_paper': return ['Conference Paper'];
    case 'thesis': return ['Thesis'];
    case 'report': return ['Technical Report'];
    case 'preprint': return ['Preprint'];
    case 'patent': return ['Patent'];
    case 'dataset': return ['Dataset'];
    case 'newspaper_article': return ['Newspaper Article'];
    case 'article': return ['Journal Article'];
    default: return [];
  }
}

/**
 * extra_metadata 里保存原始 pub_types 的私有键。
 *
 * 下划线前缀表示「非显示字段」：MetadataView 只按 getTypeFields(paper_type) 的
 * 白名单取 extra_metadata，永远不会渲染这个键。它存在的唯一目的是让
 * GET→PUT 往返（updatePaper）能把 pub_types 原样带回去。
 *
 * 只活在客户端：后端 Article 结构体没有 extra_metadata 字段且 serde 忽略未知
 * 字段，PUT 带上去会被静默丢弃、下次 GET 也不回来。它之所以闭环，是因为每次
 * 读（articleToPaper / detailToPaper）都会用服务端的 pub_types 重新填它 ——
 * 真正持久化的只有 pub_types 本身。因此任何 extra_metadata 键都不能当成
 * 「写进去就还在」的状态用，回显一律重新从 Article 顶层字段推导。
 *
 * 盲区与解除条件：paperToArticle 优先取这个键，所以它会遮蔽 paper_type 上的
 * 改动。服务端兜不住 —— PUT 收的是完整 Article JSON，无从分辨 body 里的
 * pub_types 是「用户本意」还是「缓存原样带回」，任何服务器侧校验都只能二选一
 * （拒掉合法往返，或放行静默还原）。正解在调用侧：将来放开类型编辑时，让编辑器
 * 在改 paper_type 的同时经 paperTypeToPubTypes 反向生成新数组写进这个键（或
 * 干脆清掉它），遮蔽条件就不成立，paperToArticle 一行不用动。不要改成
 * 「以 paper_type 为准」—— 那会把多标签记录塌缩成单标签，丢掉往返不失真。
 */
export const PRIVATE_PUB_TYPES_KEY = '_pub_types';

// ============================================================
// Attachment 合成
// ============================================================

/** 取路径的最后一段作为文件名（file_path 形如 `/articles/xxx.pdf`）。 */
function basenameOf(path: string): string {
  const parts = path.split('/');
  return parts[parts.length - 1] || path;
}

/** FullText.file_format → MIME type（AttachmentList 只用它做图标 / 类型提示）。 */
function mimeOfFormat(format: string | null | undefined): string {
  switch (format) {
    case 'html': return 'text/html';
    case 'txt': return 'text/plain';
    case 'pdf':
    default: return 'application/pdf';
  }
}

/**
 * autonomics 没有附件表 —— 每篇文章至多一份 fulltext 文件。
 * 这里从详情接口的 fulltext 对象合成 jayread 的单个「主附件」，让
 * AttachmentList / PaperReaderPage 的附件逻辑继续工作：
 * - 有 fulltext → 一个 `is_primary: true` 的附件，file_path 指向 /fulltext/raw
 * - 无 fulltext → 空数组（列表行不展开附件区）
 *
 * 合成的附件 id 是 `ft-<safeId>`：getAttachmentUrl(attachmentId) 只拿得到 id，
 * 把 safeId 编进去才能反查出该走哪篇文章的 fulltext。
 */
export function synthesizeAttachments(
  article: BibArticle,
  fulltext?: BibFullText | null,
): Attachment[] {
  if (!fulltext) return [];

  const safeId = encodeSafeId(article.id);
  return [
    {
      id: `ft-${safeId}`,
      paper_id: safeId,
      // fulltext 没有 filename 字段，只有 vfs 路径 —— 取最后一段当文件名
      filename: basenameOf(fulltext.file_path) || `${article.title || safeId}.pdf`,
      file_type: mimeOfFormat(fulltext.file_format),
      // 附件内容统一走 fulltext/raw；AttachmentList 从不读 file_path（只用 getAttachmentUrl）
      file_path: `/api/v1/bib/articles/${encodeIdSegment(article.id)}/fulltext/raw`,
      file_size: typeof fulltext.file_size === 'number' ? fulltext.file_size : 0,
      is_primary: true,
      // FullText 的 created_at 是 uploaded_at；缺失时回落题录时间
      created_at: fulltext.uploaded_at || article.created_at || new Date().toISOString(),
    },
  ];
}

// ============================================================
// Article → Paper
// ============================================================

/**
 * 从 `pages`（如 "123-130" / "123–130"）解析起止页。
 * 解析失败时字段缺省 —— extra_metadata 是开放对象，缺键即不显示。
 */
function splitPages(pages: string | null | undefined): { start_page?: string; end_page?: string } {
  if (!pages) return {};
  const m = String(pages).match(/^(\d+)\s*[-–—]\s*(\d+)$/);
  return m ? { start_page: m[1], end_page: m[2] } : {};
}

/** putIfPresent 的对象形式（用在展开语法里），值缺省时返回空对象。 */
function putIfPresentObj(key: string, value: unknown): Record<string, unknown> {
  return value ? { [key]: value } : {};
}

/**
 * Article JSON → jayread Paper。
 *
 * @param article 后端题录
 * @param fulltext 详情接口顶层的全文对象；列表行没有全文信息，传省略的
 *   undefined。它决定 parse_status / storage_key / item_source 这三个
 *   「这篇有没有 PDF」的派生字段。
 * @param fulltextPagination 全文分页信息（提供 total_chars → markdown_length）
 */
export function articleToPaper(
  article: BibArticle,
  fulltext?: BibFullText | null,
  fulltextPagination?: BibFullTextPagination | null,
): Paper {
  const doi = identifierOf(article, 'doi');
  const pmid = identifierOf(article, 'pmid');
  const arxiv = identifierOf(article, 'arxiv');
  const pmcid = identifierOf(article, 'pmc');
  const openalex = identifierOf(article, 'openalex');
  const hasFulltext = !!fulltext;
  const pubTypes = article.pub_types ?? [];
  const paperType = pubTypesToPaperType(pubTypes);

  // extra_metadata 是 jayread 的「类型特有字段」口袋（MetadataView 按它渲染）。
  // autonomics 把这些字段平铺在 Article 顶层，这里聚拢回去。
  const extraMetadata: Record<string, unknown> = {
    ...(article.volume ? { volume: article.volume } : {}),
    ...(article.issue ? { issue: article.issue } : {}),
    ...(article.pages ? { pages: article.pages } : {}),
    ...splitPages(article.pages),
    ...(article.keywords && article.keywords.length > 0
      ? { keywords: article.keywords.join(', ') }
      : {}),
    // Article 没有 url 列；MetadataView 的「链接」字段由 DOI 合成（仅展示用）
    ...(doi ? { url: `https://doi.org/${doi}` } : {}),
    ...(doi ? { doi } : {}),
    ...(pmid ? { pmid } : {}),
    ...(pmcid ? { pmcid } : {}),
    ...(arxiv ? { arxiv_id: arxiv } : {}),
    ...putIfPresentObj('biorxiv', identifierOf(article, 'biorxiv')),
    ...putIfPresentObj('embase', identifierOf(article, 'embase')),
    ...putIfPresentObj('s2', identifierOf(article, 's2')),
    ...(article.issn ? { issn: article.issn } : {}),
    ...(article.essn ? { essn: article.essn } : {}),
    ...(article.month ? { month: article.month } : {}),
    ...(article.language ? { language: article.language } : {}),
    // 私有载体：原始 pub_types 数组，paperToArticle 回写时优先取它
    ...(pubTypes.length > 0 ? { [PRIVATE_PUB_TYPES_KEY]: pubTypes } : {}),
  };

  return {
    // ---- ID / 标题 / 作者 ----
    id: encodeSafeId(article.id),
    title: article.title ?? '',
    authors: (article.authors ?? []).map(authorToLabel),
    subtitle: null,
    doi: doi ?? null,
    pmid: pmid ?? null,
    // IdKind 没有 ISBN（bib-types 把 ISBN 归入 kind='other'），且导入管线不产出
    isbn: null,
    openalex_id: openalex ?? null,

    // ---- 发表信息 ----
    // journal = 期刊/会议名；source = 「从哪个渠道进来的」枚举。jayread 的
    // journal_name / source 恰好也是这个语义，一一对应。
    journal_name: article.journal ?? null,
    journal_issn: article.issn ?? null,
    publication_year: typeof article.year === 'number' ? article.year : null,
    // Article 没有 publisher / url 列
    publisher: null,
    url: null,
    source: article.source ?? null,
    abstract: article.abstract_text ?? null,

    // ---- 类型 ----
    paper_type: paperType,
    // jayread 的 type 是自由文本；用第一个原始标签（如 "Review"）保留来源措辞
    type: pubTypes[0] ?? null,
    extra_metadata: extraMetadata,

    // ---- 分类（autonomics collections → jayread category_ids）----
    // Article 不携带集合成员关系；列表 / 详情调用方会在 service 层另行注入
    //（见 papersApi.getPapers 用 /collections 反查），这里给安全空值。
    category_ids: [],

    // ---- 解析管线 ----
    // autonomics 无解析管线：有 fulltext 文件即视为已解析完成，否则「无文件」。
    parse_status: hasFulltext ? 'done' : 'none',
    // 没有「抽取引擎」概念；file_format 说明的是文件而非抽取过程
    parse_engine: null,
    parse_error: null,
    storage_key: hasFulltext ? fulltext!.file_path : null,
    filename: hasFulltext ? basenameOf(fulltext!.file_path) : null,
    markdown_length: fulltextPagination && typeof fulltextPagination.total_chars === 'number'
      ? fulltextPagination.total_chars
      : (hasFulltext && typeof fulltext!.text_content === 'string'
        ? fulltext!.text_content!.length
        : null),

    // ---- 翻译 / 摘要 / 引用指标（autonomics 不提供，中性默认值）----
    title_zh: null,
    abstract_zh: null,
    translation_status: null,
    paragraphTranslationStatus: null,
    hoverTranslationEnabled: undefined,
    full_summary: null,
    summary_status: null,
    summary_error: null,
    citation_count: 0,
    impact_factor: null,
    impact_factor_5: null,
    jcr_quartile: null,
    ssci_quartile: null,
    cas_quartile: null,
    cas_quartile_base: null,
    cas_small: null,
    cas_top: false,
    cas_warning: null,
    open_access_status: null,

    // ---- 来源 ----
    // item_source 驱动 PaperReaderPage 的视图分叉：无 PDF 的记录落到 MetadataView
    //（元数据视图），有 PDF 的落到 PDF 阅读器。jayread 用 'bib_import' 标记
    // 题录导入记录，正好是「无 PDF 只看元数据」的语义，这里借用它。
    item_source: hasFulltext ? 'upload' : 'bib_import',
    source_file: null,

    created_at: article.created_at ?? new Date().toISOString(),
    updated_at: article.updated_at ?? article.created_at ?? new Date().toISOString(),
  } as Paper;
}

/**
 * `GET /articles/{enc}` 信封 → Paper + 合成附件。
 *
 * getPaper / getAttachments 都要用：全文元信息在信封顶层，必须显式传给
 * articleToPaper，否则「有 PDF 的文献」会被误判成 bib_import。
 */
export function detailToPaper(detail: BibArticleDetail): Paper & { attachments: Attachment[] } {
  const paper = articleToPaper(detail.article, detail.fulltext, detail.fulltext_pagination);
  return {
    ...paper,
    attachments: synthesizeAttachments(detail.article, detail.fulltext),
  } as Paper & { attachments: Attachment[] };
}

/**
 * jayread Paper → Article JSON（PUT /articles/{enc} 的请求体）。
 *
 * 只回写 autonomics 真正拥有的字段；jayread 产品字段（影响因子、翻译状态……）
 * 后端没有对应列，丢弃。后端会覆盖 `id` / `created_at` / `updated_at`，这里仍
 * 按读到的值回传，让请求体自描述、幂等。
 *
 * `identifiers` 由标量合成：只把非空的 doi/pmid/arxiv 写回去，其余 kind
 * （pmc / openalex / biorxiv ...） Paper 没有承载位，只能丢弃 —— 这是「往返
 * 不丢用户可见字段」与「不发明后端没有的字段」之间的取舍。
 */
export function paperToArticle(paper: Paper, realId?: string): BibArticle {
  const extra = (paper.extra_metadata ?? {}) as Record<string, unknown>;
  // jayread 的 Paper 没有 arxiv 标量字段，arxiv 一律落在 extra_metadata.arxiv_id
  const arxiv = typeof extra.arxiv_id === 'string' ? extra.arxiv_id : '';

  const identifiers: BibIdentifier[] = [];
  if (paper.doi) identifiers.push({ kind: 'doi', value: paper.doi });
  if (paper.pmid) identifiers.push({ kind: 'pmid', value: paper.pmid });
  if (arxiv) identifiers.push({ kind: 'arxiv', value: arxiv });

  const keywords = typeof extra.keywords === 'string'
    ? extra.keywords.split(/\s*,\s*/).filter(Boolean)
    : [];

  // 类型：优先还原原始 pub_types（编辑往返不失真），否则从 paper_type 规范化。
  // 优先级会遮蔽 paper_type 上的改动；今天够不着（updatePaper 只写
  // hoverTranslationEnabled，paper_type 不可编辑）。将来放开类型编辑时，让编辑器
  // 经 paperTypeToPubTypes 把新类型同步写进 _pub_types（做法与原因见
  // PRIVATE_PUB_TYPES_KEY 的 docblock），不要调整这里的优先级。
  const carriedPubTypes = extra[PRIVATE_PUB_TYPES_KEY];
  const pubTypes = Array.isArray(carriedPubTypes) && carriedPubTypes.length > 0
    ? carriedPubTypes.map(String)
    : paperTypeToPubTypes(paper.paper_type ?? paper.type ?? null);

  return {
    id: realId ?? fromSafeId(paper.id),
    title: paper.title,
    authors: (paper.authors ?? [])
      .map((label) => labelToAuthor(typeof label === 'string' ? label : String(label)))
      .filter((a): a is BibAuthor => a !== null),
    identifiers,
    abstract_text: paper.abstract ?? null,
    year: typeof paper.publication_year === 'number' ? paper.publication_year : null,
    month: typeof extra.month === 'number'
      ? extra.month
      : (typeof extra.month === 'string' && extra.month !== '' ? Number(extra.month) || null : null),
    journal: paper.journal_name ?? null,
    volume: (typeof extra.volume === 'string' && extra.volume) || null,
    issue: (typeof extra.issue === 'string' && extra.issue) || null,
    pages: (typeof extra.pages === 'string' && extra.pages) || null,
    issn: (typeof extra.issn === 'string' && extra.issn) || paper.journal_issn || null,
    essn: (typeof extra.essn === 'string' && extra.essn) || null,
    language: (typeof extra.language === 'string' && extra.language) || null,
    pub_types: pubTypes,
    keywords,
    // Paper.source 存的就是后端的来源枚举字符串；不认识时回落 manual
    source: normalizeArticleSource(paper.source),
    created_at: paper.created_at,
    updated_at: paper.updated_at,
  };
}

/** Paper.source → ArticleSource 的合法取值；不认识则 'manual'。 */
function normalizeArticleSource(source: string | null | undefined): string {
  const KNOWN = new Set([
    'pubmed', 'embase', 'crossref', 'arxiv', 'biorxiv', 'europepmc',
    'semanticscholar', 'gwascatalog', 'openalex', 'manual', 'zotero', 'unknown',
  ]);
  const value = (source ?? '').trim();
  return KNOWN.has(value) ? value : 'manual';
}

// ============================================================
// 响应信封解包
// ============================================================

/** 从 `GET /articles` 响应中取出 Article 数组（`hits` 是兼容壳，不读）。 */
export function articlesOf(res: BibArticleListResponse | null | undefined): BibArticle[] {
  if (!res) return [];
  return Array.isArray(res.articles) ? res.articles : [];
}

/**
 * 集合的最小形状。
 *
 * `GET /collections` 返回的 Collection 带 `article_ids`（后端批量注水），
 * papersApi / categoriesApi 都只依赖这两个字段。
 */
export interface BibCollectionLike {
  id: string;
  article_ids?: string[];
}

/**
 * 集合列表 → 「文章 ID → 所属集合 ID 数组」倒排表。
 *
 * Article JSON 不携带成员关系，jayread 的 `Paper.category_ids` 又是列表行的
 * 直接输入（PaperListPage 用它建 paperId → 分类映射），所以列表请求要并行拉
 * 一次 `/collections` 在这里倒排 —— 一次请求换掉按文章逐个查的 N+1。
 */
export function collectionMembershipOf(
  collections: BibCollectionLike[] | null | undefined,
): Map<string, string[]> {
  const membership = new Map<string, string[]>();
  for (const collection of collections ?? []) {
    for (const articleId of collection.article_ids ?? []) {
      const owned = membership.get(articleId);
      if (owned) owned.push(collection.id);
      else membership.set(articleId, [collection.id]);
    }
  }
  return membership;
}
