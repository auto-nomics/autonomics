/**
 * mapping.ts 单元测试
 *
 * 覆盖：
 * - safeId 编解码往返（含含 `:` `/` 的真实 DOI、Unicode、空串）
 * - 非法输入的容错（真实 ID 直接传入时原样返回）
 * - articleToPaper 的字段完整性（对照 Paper 接口逐字段断言）
 * - paperToArticle 反向合成（identifiers 聚合、结构化 Author 拆回）
 * - Attachment 合成（有 / 无 fulltext）
 * - pub_types ↔ paper_type 映射
 */

import { describe, it, expect } from 'vitest';
import {
  toSafeId,
  fromSafeId,
  encodeIdSegment,
  articleToPaper,
  detailToPaper,
  paperToArticle,
  synthesizeAttachments,
  authorToLabel,
  labelToAuthor,
  pubTypesToPaperType,
  paperTypeToPubTypes,
  articlesOf,
  collectionMembershipOf,
  PRIVATE_PUB_TYPES_KEY,
  type BibArticle,
  type BibFullText,
} from '../mapping';
import type { Paper } from '@/types';

/** Paper 接口的全部必填键（articleToPaper 一个都不能缺） */
const REQUIRED_PAPER_KEYS = [
  'id', 'title', 'authors', 'journal_name', 'publication_year', 'doi', 'abstract',
  'parse_status', 'title_zh', 'abstract_zh', 'category_ids', 'citation_count',
  'impact_factor', 'paper_type', 'translation_status', 'created_at', 'updated_at',
  'filename', 'parse_engine', 'parse_error', 'storage_key', 'subtitle', 'type',
  'extra_metadata', 'item_source', 'source_file', 'full_summary', 'summary_status',
  'summary_error', 'journal_issn', 'openalex_id', 'open_access_status', 'isbn',
  'pmid', 'url', 'publisher', 'source', 'impact_factor_5', 'jcr_quartile',
  'ssci_quartile', 'cas_quartile', 'cas_quartile_base', 'cas_small', 'cas_top',
  'cas_warning', 'markdown_length',
] as const;

/** 详情接口信封顶层的 FullText fixture */
function makeFulltext(overrides: Partial<BibFullText> = {}): BibFullText {
  return {
    article_id: 'doi:10.1000/xyz',
    file_path: '/articles/10.1000-xyz.pdf',
    file_format: 'pdf',
    text_content: 'x'.repeat(42_000),
    source: 'user_upload',
    file_hash: null,
    file_size: 1_234_567,
    uploaded_at: '2026-03-01T10:00:00Z',
    ...overrides,
  };
}

/** autonomics 后端返回形状的 Article fixture（字段与 Rust Article 一一对应） */
function makeArticle(overrides: Partial<BibArticle> = {}): BibArticle {
  return {
    id: 'doi:10.1000/xyz',
    title: 'A Study of Mapping Layers',
    authors: [
      { last_name: 'Chen', fore_name: 'Alice', initials: null, affiliation: null, orcid: null, corresponding: false },
      { last_name: 'Li', fore_name: 'Bo', initials: null, affiliation: null, orcid: null, corresponding: false },
    ],
    identifiers: [
      { kind: 'doi', value: '10.1000/xyz' },
      { kind: 'pmid', value: '36112233' },
      { kind: 'arxiv', value: '2401.01234' },
      { kind: 'openalex', value: 'W1234567890' },
    ],
    abstract_text: 'We study mapping layers.',
    year: 2024,
    month: 5,
    journal: 'Journal of Interoperability',
    volume: '12',
    issue: '3',
    pages: '100-110',
    issn: '1234-5678',
    essn: '8765-4321',
    language: 'eng',
    pub_types: ['Journal Article', 'Review'],
    keywords: ['interoperability', 'mapping'],
    source: 'pubmed',
    created_at: '2026-01-15T08:00:00Z',
    updated_at: '2026-02-20T09:30:00Z',
    ...overrides,
  };
}

// ============================================================================
// safeId 编解码
// ============================================================================

describe('safeId 编解码', () => {
  it('doi:10.1000/x 往返一致', () => {
    const real = 'doi:10.1000/x';
    const safe = toSafeId(real);
    // `:` 和 `/` 都不能出现在 base64url 输出里
    expect(safe).not.toMatch(/[:/=]/);
    expect(safe).toMatch(/^[A-Za-z0-9_-]+$/);
    expect(fromSafeId(safe)).toBe(real);
  });

  it('含斜杠与中文的真实 ID 往返一致（UTF-8）', () => {
    const cases = [
      'doi:10.1000/abc.def/ghi',
      'local:9f8e7d6c-1111-2222-3333-444455556666',
      'arxiv:2401.01234v2',
      'doi:10.1000/中文标题',
      'pmid:36112233',
      'a',
    ];
    for (const real of cases) {
      expect(fromSafeId(toSafeId(real))).toBe(real);
    }
  });

  it('无 padding（长度不是 4 的倍数也合法）', () => {
    // 'doi:' 编码后长度对 4 取模为 1，必然需要 padding —— 必须被剥掉
    const safe = toSafeId('doi:');
    expect(safe.endsWith('=')).toBe(false);
    expect(fromSafeId(safe)).toBe('doi:');
  });

  it('空串原样返回', () => {
    expect(toSafeId('')).toBe('');
    expect(fromSafeId('')).toBe('');
  });

  it('真实 ID 直接传入 fromSafeId 时原样返回（容错）', () => {
    // 含 `:` 必然不是 base64url，应原样返回而不是解码出乱码
    expect(fromSafeId('doi:10.1000/xyz')).toBe('doi:10.1000/xyz');
    // 含 base64url 之外的字节（如 `%`）
    expect(fromSafeId('weird%id')).toBe('weird%id');
  });

  it('encodeIdSegment 百分号编码真实 ID（wire 格式）', () => {
    expect(encodeIdSegment('doi:10.1000/xyz')).toBe('doi%3A10.1000%2Fxyz');
    // 后端按字面解码路径段，safeId 不用于 wire
    expect(encodeIdSegment(fromSafeId(toSafeId('doi:10.1000/xyz')))).toBe('doi%3A10.1000%2Fxyz');
  });
});

// ============================================================================
// 结构化 Author ↔ 显示字符串
// ============================================================================

describe('authorToLabel / labelToAuthor', () => {
  it('fore_name + last_name 拼成自然阅读顺序', () => {
    expect(authorToLabel({ last_name: 'Chen', fore_name: 'Alice' })).toBe('Alice Chen');
    expect(authorToLabel(makeArticle().authors[0])).toBe('Alice Chen');
  });

  it('只有姓时不产生前导空格', () => {
    expect(authorToLabel({ last_name: 'Cher', fore_name: null })).toBe('Cher');
    expect(authorToLabel({ last_name: 'Cher', fore_name: '' })).toBe('Cher');
  });

  it('字符串输入原样透传（防御后端字段变化）', () => {
    expect(authorToLabel('Plain String')).toBe('Plain String');
  });

  it('拆回：最后一个词是姓，其余是名', () => {
    expect(labelToAuthor('Alice Chen')).toMatchObject({ fore_name: 'Alice', last_name: 'Chen' });
    // 多词姓按启发式归名，往返仍无损
    const round = labelToAuthor('Willy van der Berg');
    expect(round?.fore_name).toBe('Willy van der');
    expect(round?.last_name).toBe('Berg');
    expect(authorToLabel(round!)).toBe('Willy van der Berg');
  });

  it('单名作者只有一个词，全部归姓', () => {
    expect(labelToAuthor('Cher')).toMatchObject({ last_name: 'Cher', fore_name: null });
  });

  it('空白串返回 null（不留空作者行）', () => {
    expect(labelToAuthor('')).toBeNull();
    expect(labelToAuthor('   ')).toBeNull();
  });
});

// ============================================================================
// articleToPaper
// ============================================================================

describe('articleToPaper', () => {
  it('产出 Paper 接口的全部字段，一个不缺', () => {
    const paper = articleToPaper(makeArticle()) as unknown as Record<string, unknown>;
    for (const key of REQUIRED_PAPER_KEYS) {
      expect(paper, `缺少字段 ${key}`).toHaveProperty(key);
    }
  });

  it('id 是 safeId（无 `:` `/`），真实 ID 可还原', () => {
    const paper = articleToPaper(makeArticle());
    expect(paper.id).not.toMatch(/[:/]/);
    expect(fromSafeId(paper.id)).toBe('doi:10.1000/xyz');
  });

  it('identifiers 数组拆成标量，冷门 kind 落进 extra_metadata', () => {
    const paper = articleToPaper(makeArticle());
    expect(paper.doi).toBe('10.1000/xyz');
    expect(paper.pmid).toBe('36112233');
    expect(paper.openalex_id).toBe('W1234567890');
    // IdKind 没有 ISBN（后端归入 kind='other'），Paper.isbn 恒为 null
    expect(paper.isbn).toBeNull();
    // arxiv 落在 extra_metadata.arxiv_id（Paper 无 arxiv 标量字段）
    const extra = paper.extra_metadata as Record<string, unknown>;
    expect(extra.arxiv_id).toBe('2401.01234');
  });

  it('基本元数据一一对应', () => {
    const paper = articleToPaper(makeArticle());
    expect(paper.title).toBe('A Study of Mapping Layers');
    expect(paper.authors).toEqual(['Alice Chen', 'Bo Li']);
    expect(paper.abstract).toBe('We study mapping layers.');
    expect(paper.publication_year).toBe(2024);
    expect(paper.created_at).toBe('2026-01-15T08:00:00Z');
    expect(paper.updated_at).toBe('2026-02-20T09:30:00Z');
  });

  it('journal → journal_name，source 枚举 → source', () => {
    const paper = articleToPaper(makeArticle());
    expect(paper.journal_name).toBe('Journal of Interoperability');
    expect(paper.journal_issn).toBe('1234-5678');
    // source 是「来源渠道」枚举，不是期刊名
    expect(paper.source).toBe('pubmed');
  });

  it('Article 没有 publisher / url 列，对应字段为 null', () => {
    const paper = articleToPaper(makeArticle());
    expect(paper.publisher).toBeNull();
    expect(paper.url).toBeNull();
    // MetadataView 的「链接」由 DOI 合成，只落在 extra_metadata
    expect((paper.extra_metadata as Record<string, unknown>).url).toBe('https://doi.org/10.1000/xyz');
  });

  it('Article 不携带集合成员关系，category_ids 恒为空', () => {
    // 成员关系由 papersApi.getPapers 用 /collections 倒排注入
    expect(articleToPaper(makeArticle()).category_ids).toEqual([]);
  });

  it('有 fulltext 时 parse_status=done 且派生出文件元信息', () => {
    const paper = articleToPaper(makeArticle(), makeFulltext());
    expect(paper.parse_status).toBe('done');
    expect(paper.filename).toBe('10.1000-xyz.pdf');
    expect(paper.storage_key).toBe('/articles/10.1000-xyz.pdf');
    expect(paper.parse_engine).toBeNull(); // 后端没有抽取引擎概念
    expect(paper.markdown_length).toBe(42_000);
    expect(paper.item_source).toBe('upload');
  });

  it('markdown_length 优先取分页的 total_chars', () => {
    const paper = articleToPaper(makeArticle(), makeFulltext({ text_content: 'abc' }), {
      offset: 0, limit: 1, total_chars: 99_999, truncated: true, next_offset: 3,
    });
    expect(paper.markdown_length).toBe(99_999);
  });

  it('无 fulltext 时 parse_status=none 且管线字段为 null', () => {
    const paper = articleToPaper(makeArticle());
    expect(paper.parse_status).toBe('none');
    expect(paper.filename).toBeNull();
    expect(paper.storage_key).toBeNull();
    expect(paper.parse_engine).toBeNull();
    expect(paper.markdown_length).toBeNull();
    expect(paper.item_source).toBe('bib_import');
  });

  it('有 / 无 fulltext 决定 PaperReaderPage 的视图分叉', () => {
    // PaperReaderPage 用 item_source === 'bib_import' && 无 storage_key && 无附件
    // 决定显示 MetadataView；autonomics 无 PDF 的记录正好需要这个分支
    const withPdf = articleToPaper(makeArticle(), makeFulltext());
    const withoutPdf = articleToPaper(makeArticle());
    expect(withPdf.item_source).not.toBe('bib_import');
    expect(withoutPdf.item_source).toBe('bib_import');
    expect(withoutPdf.storage_key).toBeNull();
  });

  it('fulltext 行自带的解析状态（MinerU 列）优先于旧语义回落', () => {
    const paper = articleToPaper(makeArticle(), makeFulltext({
      parse_status: 'processing',
      parse_engine: null,
      parse_error: null,
    }));
    expect(paper.parse_status).toBe('processing');

    const failed = articleToPaper(makeArticle(), makeFulltext({
      parse_status: 'failed',
      parse_engine: 'mineru',
      parse_error: 'MinerU parse failed: quota',
    }));
    expect(failed.parse_status).toBe('failed');
    expect(failed.parse_engine).toBe('mineru');
    expect(failed.parse_error).toBe('MinerU parse failed: quota');
  });

  it('/fulltext-statuses 的倒排行（第 4 参）优先级最高，覆盖 fulltext 行', () => {
    const paper = articleToPaper(
      makeArticle(),
      makeFulltext({ parse_status: 'done', parse_engine: 'builtin' }),
      undefined,
      { parse_status: 'pending', parse_engine: null, parse_error: null },
    );
    expect(paper.parse_status).toBe('pending');
    expect(paper.parse_engine).toBeNull(); // 覆盖为倒排行的值，不保留 fulltext 的 builtin
  });

  it('autonomics 没有的产品字段填中性默认值', () => {
    const paper = articleToPaper(makeArticle());
    expect(paper.title_zh).toBeNull();
    expect(paper.abstract_zh).toBeNull();
    expect(paper.translation_status).toBeNull();
    expect(paper.citation_count).toBe(0);
    expect(paper.impact_factor).toBeNull();
    expect(paper.impact_factor_5).toBeNull();
    expect(paper.jcr_quartile).toBeNull();
    expect(paper.ssci_quartile).toBeNull();
    expect(paper.cas_quartile).toBeNull();
    expect(paper.cas_top).toBe(false);
    expect(paper.full_summary).toBeNull();
    expect(paper.open_access_status).toBeNull();
    expect(paper.parse_error).toBeNull();
  });

  it('顶层字段聚进 extra_metadata 供 MetadataView 渲染', () => {
    const extra = articleToPaper(makeArticle()).extra_metadata as Record<string, unknown>;
    expect(extra.volume).toBe('12');
    expect(extra.issue).toBe('3');
    expect(extra.pages).toBe('100-110');
    expect(extra.start_page).toBe('100');
    expect(extra.end_page).toBe('110');
    expect(extra.keywords).toBe('interoperability, mapping');
    expect(extra.issn).toBe('1234-5678');
    expect(extra.essn).toBe('8765-4321');
    expect(extra.month).toBe(5);
    expect(extra.language).toBe('eng');
  });

  it('原始 pub_types 存进私有键，且不落在任何显示白名单里', () => {
    const extra = articleToPaper(makeArticle()).extra_metadata as Record<string, unknown>;
    expect(extra[PRIVATE_PUB_TYPES_KEY]).toEqual(['Journal Article', 'Review']);
    expect(String(PRIVATE_PUB_TYPES_KEY)).toMatch(/^_/);
  });

  it('快照：完整映射结果稳定', () => {
    const paper = articleToPaper(makeArticle(), makeFulltext());
    expect(paper).toMatchObject({
      title: 'A Study of Mapping Layers',
      doi: '10.1000/xyz',
      pmid: '36112233',
      publication_year: 2024,
      parse_status: 'done',
      paper_type: 'review',
      category_ids: [],
      citation_count: 0,
      markdown_length: 42_000,
      filename: '10.1000-xyz.pdf',
    });
  });
});

// ============================================================================
// detailToPaper（详情信封）
// ============================================================================

describe('detailToPaper', () => {
  it('从信封顶层取 fulltext 并挂上合成附件', () => {
    const paper = detailToPaper({
      article: makeArticle(),
      fulltext: makeFulltext(),
      fulltext_pagination: { offset: 0, limit: 1, total_chars: 42_000, truncated: true, next_offset: 1 },
      annotations: [],
    });
    expect(paper.parse_status).toBe('done');
    expect(paper.attachments).toHaveLength(1);
    expect(paper.attachments[0].is_primary).toBe(true);
  });

  it('fulltext 为 null 时仍是合法 Paper，附件为空数组', () => {
    const paper = detailToPaper({ article: makeArticle(), fulltext: null, fulltext_pagination: null, annotations: [] });
    expect(paper.parse_status).toBe('none');
    expect(paper.attachments).toEqual([]);
  });
});

// ============================================================================
// paperToArticle
// ============================================================================

describe('paperToArticle', () => {
  it('非空标识符合成为 identifiers[]（kind: doi/pmid/arxiv）', () => {
    const paper = articleToPaper(makeArticle());
    const article = paperToArticle(paper);

    expect(article.identifiers).toEqual(
      expect.arrayContaining([
        { kind: 'doi', value: '10.1000/xyz' },
        { kind: 'pmid', value: '36112233' },
        { kind: 'arxiv', value: '2401.01234' },
      ]),
    );
  });

  it('空标识符不产生空值条目', () => {
    const paper = articleToPaper(makeArticle({
      identifiers: [],
      keywords: [],
      volume: null,
      issue: null,
      pages: null,
    }));
    const article = paperToArticle(paper);

    expect(article.identifiers).toEqual([]);
    expect(article.volume).toBeNull();
    expect(article.issue).toBeNull();
    expect(article.pages).toBeNull();
    expect(article.keywords).toEqual([]);
  });

  it('extra_metadata 的 volume/issue/pages 回写到顶层字段', () => {
    const paper = articleToPaper(makeArticle());
    const article = paperToArticle(paper);
    expect(article.volume).toBe('12');
    expect(article.issue).toBe('3');
    expect(article.pages).toBe('100-110');
  });

  it('作者字符串拆回结构化 Author', () => {
    const paper = articleToPaper(makeArticle());
    const article = paperToArticle(paper);
    expect(article.authors).toEqual([
      expect.objectContaining({ fore_name: 'Alice', last_name: 'Chen' }),
      expect.objectContaining({ fore_name: 'Bo', last_name: 'Li' }),
    ]);
  });

  it('pub_types 优先还原原始数组（编辑往返不失真）', () => {
    const paper = articleToPaper(makeArticle());
    // paper_type 已收敛成单值 'review'，但回写应带原始双标签
    expect(paper.paper_type).toBe('review');
    expect(paperToArticle(paper).pub_types).toEqual(['Journal Article', 'Review']);
  });

  it('没有原始 pub_types 时从 paper_type 规范化', () => {
    const paper = {
      ...articleToPaper(makeArticle({ pub_types: [] })),
      extra_metadata: {},
      paper_type: 'conference_paper',
    };
    expect(paperToArticle(paper).pub_types).toEqual(['Conference Paper']);
  });

  it('journal_name 回写 journal；来源枚举透传', () => {
    const paper = articleToPaper(makeArticle());
    const article = paperToArticle(paper);
    expect(article.journal).toBe('Journal of Interoperability');
    expect(article.source).toBe('pubmed');
  });

  it('不认识的 source 回落 manual（后端枚举封闭）', () => {
    const paper = { ...articleToPaper(makeArticle()), source: 'some-future-source' };
    expect(paperToArticle(paper).source).toBe('manual');
  });

  it('abstract 映射到 abstract_text（Rust 字段名）', () => {
    const paper = articleToPaper(makeArticle());
    expect(paperToArticle(paper).abstract_text).toBe('We study mapping layers.');
  });

  it('指定 realId 时用它作为文章 ID（safeId 不进请求体）', () => {
    const paper = articleToArticlePaper();
    const article = paperToArticle(paper, 'doi:10.1000/xyz');
    expect(article.id).toBe('doi:10.1000/xyz');
  });

  /** articleToPaper 出来的 paper，safeId 形态的 id */
  function articleToArticlePaper(): Paper {
    return articleToPaper(makeArticle());
  }

  it('GET→PUT 往返幂等（题录字段不变）', () => {
    const paper = articleToPaper(makeArticle(), makeFulltext());
    const once = paperToArticle(paper, 'doi:10.1000/xyz');
    const twice = paperToArticle(articleToPaper(once), 'doi:10.1000/xyz');
    expect(twice).toEqual(once);
  });
});

// ============================================================================
// pub_types ↔ paper_type
// ============================================================================

describe('pubTypesToPaperType / paperTypeToPubTypes', () => {
  it('常见 PublicationType 正向映射', () => {
    expect(pubTypesToPaperType(['Journal Article'])).toBe('article');
    expect(pubTypesToPaperType(['Review'])).toBe('review');
    expect(pubTypesToPaperType(['Meta-Analysis'])).toBe('review');
    expect(pubTypesToPaperType(['Preprint'])).toBe('preprint');
    expect(pubTypesToPaperType(['Congresses'])).toBe('conference_paper');
    expect(pubTypesToPaperType(['Thesis'])).toBe('thesis');
    expect(pubTypesToPaperType(['Technical Report'])).toBe('report');
    expect(pubTypesToPaperType(['Patent'])).toBe('patent');
    expect(pubTypesToPaperType(['Book'])).toBe('book');
  });

  it('chapter 优先于 book（["Book Chapter"] 不能映射成 book）', () => {
    expect(pubTypesToPaperType(['Book Chapter'])).toBe('book_section');
  });

  it('多标签取第一个命中的规则', () => {
    // Review 比 Journal Article 更特殊
    expect(pubTypesToPaperType(['Journal Article', 'Review'])).toBe('review');
  });

  it('空 / 未知回落 article', () => {
    expect(pubTypesToPaperType([])).toBe('article');
    expect(pubTypesToPaperType(null)).toBe('article');
    expect(pubTypesToPaperType(['Something Entirely New'])).toBe('article');
  });

  it('paperTypeToPubTypes 与正向映射对已知项互逆', () => {
    for (const paperType of ['article', 'review', 'book', 'book_section', 'conference_paper', 'thesis', 'preprint']) {
      expect(pubTypesToPaperType(paperTypeToPubTypes(paperType))).toBe(paperType);
    }
  });
});

// ============================================================================
// Attachment 合成
// ============================================================================

describe('synthesizeAttachments', () => {
  it('有 fulltext 时合成单个主附件，文件名取路径末段', () => {
    const article = makeArticle();
    const [att] = synthesizeAttachments(article, makeFulltext({ file_path: '/articles/paper-final.pdf', file_size: 2048 }));

    expect(att).toBeDefined();
    expect(att.is_primary).toBe(true);
    expect(att.paper_id).toBe(toSafeId('doi:10.1000/xyz'));
    expect(att.id).toBe(`ft-${toSafeId('doi:10.1000/xyz')}`);
    expect(att.filename).toBe('paper-final.pdf');
    expect(att.file_type).toBe('application/pdf');
    expect(att.file_size).toBe(2048);
    expect(att.created_at).toBe('2026-03-01T10:00:00Z'); // uploaded_at
    // 附件内容指向 fulltext/raw，且 URL 里是百分号编码的真实 ID
    expect(att.file_path).toBe('/api/v1/bib/articles/doi%3A10.1000%2Fxyz/fulltext/raw');
  });

  it('file_format 决定 MIME type', () => {
    const article = makeArticle();
    expect(synthesizeAttachments(article, makeFulltext({ file_format: 'html' }))[0].file_type).toBe('text/html');
    expect(synthesizeAttachments(article, makeFulltext({ file_format: 'txt' }))[0].file_type).toBe('text/plain');
  });

  it('uploaded_at 缺失时回落题录 created_at', () => {
    const [att] = synthesizeAttachments(makeArticle(), makeFulltext({ uploaded_at: null }));
    expect(att.created_at).toBe('2026-01-15T08:00:00Z');
  });

  it('无 fulltext 时返回空数组', () => {
    expect(synthesizeAttachments(makeArticle())).toEqual([]);
    expect(synthesizeAttachments(makeArticle(), null)).toEqual([]);
  });

  it('合成 id 可作为 URL query 传递（无保留字符）', () => {
    const [att] = synthesizeAttachments(makeArticle(), makeFulltext());
    expect(att.id).toMatch(/^ft-[A-Za-z0-9_-]+$/);
  });
});

// ============================================================================
// articlesOf / collectionMembershipOf（响应信封解包）
// ============================================================================

describe('articlesOf', () => {
  it('读 articles 数组', () => {
    const a = makeArticle();
    expect(articlesOf({ total: 1, articles: [a] })).toEqual([a]);
  });

  it('hits 是兼容壳（score 恒 0、snippet 恒等于 title），不读', () => {
    const a = makeArticle();
    expect(articlesOf({ total: 1, hits: [{ article_id: a.id, title: a.title, score: 0, snippet: a.title }] })).toEqual([]);
  });

  it('null / undefined / 空对象返回空数组', () => {
    expect(articlesOf(null)).toEqual([]);
    expect(articlesOf(undefined)).toEqual([]);
    expect(articlesOf({ total: 0 })).toEqual([]);
  });
});

describe('collectionMembershipOf', () => {
  it('把集合的 article_ids 倒排成 文章 → 集合[]', () => {
    const membership = collectionMembershipOf([
      { id: 'col-a', article_ids: ['doi:1', 'doi:2'] },
      { id: 'col-b', article_ids: ['doi:2'] },
    ]);
    expect(membership.get('doi:1')).toEqual(['col-a']);
    expect(membership.get('doi:2')).toEqual(['col-a', 'col-b']);
    expect(membership.get('doi:3')).toBeUndefined();
  });

  it('空 / 异常输入返回空表', () => {
    expect(collectionMembershipOf(null).size).toBe(0);
    expect(collectionMembershipOf([]).size).toBe(0);
    expect(collectionMembershipOf([{ id: 'col-x' }]).size).toBe(0);
  });
});
