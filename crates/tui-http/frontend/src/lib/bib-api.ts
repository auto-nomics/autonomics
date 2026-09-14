export interface Identifier {
  kind: string;
  value: string;
}

export interface Author {
  last_name: string;
  fore_name?: string | null;
  initials?: string | null;
}

export interface Article {
  id: string;
  title: string;
  authors: Author[];
  identifiers: Identifier[];
  abstract_text?: string | null;
  year?: number | null;
  journal?: string | null;
  volume?: string | null;
  issue?: string | null;
  pages?: string | null;
  source?: string;
}

export interface SearchHit {
  article_id: string;
  title: string;
  snippet: string;
}

export interface Annotation {
  id: string;
  kind: string;
  content: string;
  page?: number | null;
  created_at?: string | null;
}

export interface FullText {
  article_id: string;
  file_path: string;
  file_format: string;
  text_content?: string | null;
  source: string;
  file_hash?: string | null;
  file_size?: number | null;
  /** Extraction lifecycle: pending | running | done | failed. */
  extract_status?: string | null;
  /** Layout of `text_content`: plain | markdown. */
  text_format?: string | null;
  /** Which extractor produced the text (mineru, simple, …). */
  extracted_by?: string | null;
  /** Failure message when `extract_status` is "failed". */
  extract_error?: string | null;
}

export interface ArticleDetail {
  article: Article;
  fulltext?: FullText | null;
  annotations: Annotation[];
}

export interface SourceBatch {
  source: string;
  total: number;
  articles: Article[];
  error?: string;
}

export interface Collection {
  id: string;
  name: string;
  article_ids: string[];
  status: string;
}

export interface BibHealth {
  status: string;
  article_count: number;
  sources: string[];
}

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, init);
  const data = await response.json().catch(() => null);
  if (!response.ok) {
    const message = data && typeof data.error === "string" ? data.error : response.statusText;
    throw new Error(message);
  }
  return data as T;
}

/** Queue a background re-extraction of a stored full text. */
export async function reextractFulltext(articleId: string): Promise<FullText> {
  const data = await api<{ fulltext: FullText }>(
    `/api/v1/bib/articles/${encodeURIComponent(articleId)}/fulltext/reextract`,
    { method: "POST" },
  );
  return data.fulltext;
}

export function articleIdentifier(article: Article, kinds: string[]): Identifier | undefined {
  return article.identifiers.find((identifier) => kinds.includes(identifier.kind));
}

export function identifierLabel(article: Article): string {
  const identifier =
    articleIdentifier(article, ["doi", "pmid", "arxiv", "pmc", "openalex", "s2"]) ??
    article.identifiers[0];
  return identifier ? `${identifier.kind}:${identifier.value}` : article.id;
}

export function authorLine(article: Article): string {
  if (!article.authors.length) return "Unknown author";
  const names = article.authors.slice(0, 4).map((author) => {
    const given = author.fore_name ?? author.initials ?? "";
    return given ? `${author.last_name} ${given}` : author.last_name;
  });
  return names.length === article.authors.length ? names.join(", ") : `${names.join(", ")} et al.`;
}
