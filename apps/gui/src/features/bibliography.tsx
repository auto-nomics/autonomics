import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { ExternalLink } from "lucide-react";
import { gatewayRequest, pickUploadFile, saveTextFile, uploadBibFulltext } from "@/gateway";
import { openPdfReader } from "@/pdf-reader";
import type { BibArticle, BibCollection, BibDetail } from "@/types";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { buttonVariants } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

type BibAuthor = NonNullable<BibArticle["authors"]>[number];

type MetadataRowProps = {
  label: string;
  value?: string | number | null;
  href?: string | null;
  children?: ReactNode;
};

function MetadataTable({ children }: { children: ReactNode }) {
  return (
    <div className="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-2 gap-y-0.5">
      {children}
    </div>
  );
}

function authorName(author: BibAuthor) {
  if (author.name) return author.name;
  return [author.last_name, author.fore_name].filter(Boolean).join(", ");
}

function articleAuthors(article: BibArticle) {
  return (article.authors ?? [])
    .map(authorName)
    .filter(Boolean)
    .join(", ");
}

function articleIdentifier(article: BibArticle, kind: string) {
  return article.identifiers?.find((item) => item.kind.toLowerCase() === kind)?.value ?? null;
}

function encodedUrlPath(value: string) {
  return value
    .replace(/#/g, "%23")
    .replace(/\?/g, "%3f")
    .replace(/%/g, "%25")
    .replace(/"/g, "%22");
}

function identifierHref(kind: string, value: string) {
  const normalizedKind = kind.toLowerCase();

  if (normalizedKind === "doi") {
    const doi = value.match(/10\.\d{4,9}\/\S+/i)?.[0];
    return doi ? `https://doi.org/${encodedUrlPath(doi)}` : null;
  }
  if (normalizedKind === "pmid" && /^\d+$/.test(value)) {
    return `https://pubmed.ncbi.nlm.nih.gov/${value}/`;
  }
  if (normalizedKind === "pmc") {
    const pmcid = /^PMC\d+$/i.test(value) ? value.toUpperCase() : `PMC${value}`;
    return /^PMC\d+$/.test(pmcid)
      ? `https://pmc.ncbi.nlm.nih.gov/articles/${pmcid}/`
      : null;
  }
  if (normalizedKind === "arxiv") {
    return `https://arxiv.org/abs/${value.replace(/^arxiv:/i, "")}`;
  }
  if (normalizedKind === "openalex" && /^[Ww]\d+$/.test(value)) {
    return `https://openalex.org/${value.toUpperCase()}`;
  }
  return null;
}

function MetadataRow({ label, value, href, children }: MetadataRowProps) {
  if (!value && !children) return null;

  return (
    <div className="group/row col-span-2 grid grid-cols-[subgrid]">
      <div className="pr-2 text-right text-[13px] leading-6 text-muted-foreground">
        {label}
      </div>
      <div className="flex min-w-0 items-start gap-1">
        <div
          className="min-w-0 flex-1 rounded-[5px] px-1 py-[2px] leading-6 transition-colors group-hover/row:bg-accent/40"
          aria-label={label}
        >
          {children ?? (
            <span className="text-[13px] break-words whitespace-pre-wrap">{value}</span>
          )}
        </div>
        {href && (
          <a
            href={href}
            target="_blank"
            rel="noreferrer"
            aria-label={`Open ${label}`}
            className={cn(
              buttonVariants({ variant: "ghost", size: "icon-xs" }),
              "mt-px opacity-0 transition-opacity group-hover/row:opacity-100 focus-visible:opacity-100",
            )}
          >
            <ExternalLink />
          </a>
        )}
      </div>
    </div>
  );
}

export function BibliographyView() {
  const [collections, setCollections] = useState<BibCollection[]>([]);
  const [articles, setArticles] = useState<BibArticle[]>([]);
  const [total, setTotal] = useState(0);
  const [queryDraft, setQueryDraft] = useState("");
  const [query, setQuery] = useState("");
  const [selectedCollection, setSelectedCollection] = useState<string | null>(null);
  const [selectedArticle, setSelectedArticle] = useState<string | null>(null);
  const [detail, setDetail] = useState<BibDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const loadArticles = useCallback(async (nextQuery = query) => {
    setLoading(true);
    setError(null);
    try {
      const [collectionResponse, articleResponse] = await Promise.all([
        gatewayRequest("GET", "/bib/collections"),
        gatewayRequest("GET", `/bib/articles?query=${encodeURIComponent(nextQuery)}&limit=500`),
      ]);
      const nextCollections: BibCollection[] = collectionResponse.collections ?? [];
      const nextArticles: BibArticle[] = articleResponse.articles ?? [];
      setCollections(nextCollections);
      setArticles(nextArticles);
      setTotal(articleResponse.total ?? nextArticles.length);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadArticles("");
  }, [loadArticles]);

  const visibleArticles = useMemo(() => {
    if (!selectedCollection) return articles;
    const members = new Set(
      collections.find((collection) => collection.id === selectedCollection)?.article_ids ?? [],
    );
    return articles.filter((article) => members.has(article.id));
  }, [articles, collections, selectedCollection]);

  const selectArticle = useCallback((articleId: string | null) => {
    setSelectedArticle(articleId);
    setDetail(null);
    if (!articleId) return;
    setDetailLoading(true);
    void gatewayRequest("GET", `/bib/articles/${encodeURIComponent(articleId)}`)
      .then((response) => setDetail(response as BibDetail))
      .catch((cause) => setError(String(cause)))
      .finally(() => setDetailLoading(false));
  }, []);

  const selected = visibleArticles.find((article) => article.id === selectedArticle)
    ?? detail?.article
    ?? null;
  const activeDetail = selected && detail?.article.id === selected.id ? detail : null;
  const headerByline = selected
    ? [
        articleAuthors(selected).split(", ")[0],
        selected.year ? `(${selected.year})` : "",
      ].filter(Boolean).join(" ")
    : "";

  const importArticle = async () => {
    const identifier = window.prompt("DOI, PMID, arXiv, or OpenAlex ID");
    if (!identifier) return;
    setBusy("import");
    setError(null);
    try {
      const response = await gatewayRequest("POST", "/bib/articles/import", {
        id_type: identifier.startsWith("doi:") ? "doi" : "doi",
        id: identifier,
        source: null,
        fetch_fulltext: true,
      });
      await loadArticles();
      const articleId = response.article?.id;
      if (articleId) selectArticle(articleId);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(null);
    }
  };

  const exportLibrary = async () => {
    setBusy("export");
    setError(null);
    try {
      const response = await gatewayRequest("GET", "/bib/export?format=bibtex&limit=500");
      await saveTextFile("autonomics.bib", response.export ?? "");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(null);
    }
  };

  const uploadFulltext = async () => {
    if (!selected) return;
    const file = await pickUploadFile();
    if (!file) return;
    setBusy("upload");
    setError(null);
    try {
      await uploadBibFulltext(selected.id, file);
      selectArticle(selected.id);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(null);
    }
  };

  const readPdf = () => {
    if (!selected) return;
    void openPdfReader({ articleId: selected.id, title: selected.title }).catch((cause) => setError(String(cause)));
  };

  const identifierRows = [
    { kind: "doi", label: "DOI" },
    { kind: "pmid", label: "PMID" },
    { kind: "pmc", label: "PMCID" },
    { kind: "arxiv", label: "arXiv" },
    { kind: "openalex", label: "OpenAlex" },
  ];

  return (
    <div className="grid min-h-0 flex-1 gap-3 xl:grid-cols-[240px_360px_minmax(0,1fr)]">
      <div className="flex min-h-0 flex-col gap-2">
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-medium">Collections</h2>
          <Badge variant="secondary">{collections.length}</Badge>
        </div>
        <Card className="min-h-0 flex-1 overflow-auto p-1.5">
          <button
            type="button"
            onClick={() => setSelectedCollection(null)}
            className={cn(
              "w-full rounded-md px-2.5 py-2 text-left text-sm hover:bg-accent",
              !selectedCollection && "bg-accent text-accent-foreground",
            )}
          >
            <span className="block font-medium">All articles</span>
            <span className="text-xs text-muted-foreground">{total.toLocaleString()}</span>
          </button>
          {collections.map((collection) => (
            <button
              key={collection.id}
              type="button"
              title={collection.description ?? collection.name}
              onClick={() => setSelectedCollection(collection.id)}
              className={cn(
                "mt-1 w-full rounded-md px-2.5 py-2 text-left text-sm hover:bg-accent",
                selectedCollection === collection.id && "bg-accent text-accent-foreground",
              )}
            >
              <span className="line-clamp-2 block font-medium">{collection.name}</span>
              <span className="text-xs text-muted-foreground">
                {collection.article_ids.length.toLocaleString()} articles
              </span>
            </button>
          ))}
        </Card>
      </div>

      <div className="flex min-h-0 flex-col gap-2">
        <form
          className="flex gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            setQuery(queryDraft);
            void loadArticles(queryDraft);
          }}
        >
          <Input
            value={queryDraft}
            placeholder={`Search ${total.toLocaleString()} articles`}
            onChange={(event) => setQueryDraft(event.target.value)}
          />
          <Button
            type="submit"
            disabled={loading}
          >
            Search
          </Button>
        </form>
        <Card className="min-h-0 flex-1 overflow-auto p-1.5">
          {visibleArticles.map((article) => (
            <button
              key={article.id}
              type="button"
              onClick={() => selectArticle(article.id)}
              className={cn(
                "mb-1 w-full rounded-md border border-transparent px-2.5 py-2 text-left hover:bg-accent",
                selectedArticle === article.id && "border-primary/60 bg-accent",
              )}
            >
              <span className="line-clamp-2 block text-sm font-medium">{article.title}</span>
              <span className="mt-1 block truncate text-xs text-muted-foreground">
                {article.year ?? "----"} · {articleAuthors(article) || article.source || "unknown"}
              </span>
            </button>
          ))}
          {!visibleArticles.length && !loading && (
            <div className="grid h-full place-items-center text-sm text-muted-foreground">No matching articles</div>
          )}
        </Card>
      </div>

      <Card className="flex min-h-0 flex-col overflow-hidden p-0">
        <div className="flex items-start justify-between gap-3 border-b p-4">
          <div className="min-w-0">
            <h2 className="line-clamp-4 text-base leading-[1.333] font-semibold break-words">
              {selected?.title ?? "Bibliography"}
            </h2>
            {selected && headerByline && (
              <p className="mt-1 text-[13px] text-muted-foreground">{headerByline}</p>
            )}
          </div>
          <div className="flex shrink-0 flex-wrap justify-end gap-2">
            {detailLoading && <Badge variant="secondary">Loading…</Badge>}
            {activeDetail?.fulltext?.file_format?.toLowerCase() === "pdf" && (
              <Button size="sm" variant="outline" onClick={readPdf}>Read PDF</Button>
            )}
            <Button size="sm" variant="secondary" onClick={() => void importArticle()} disabled={busy === "import"}>
              Import
            </Button>
            <Button size="sm" variant="secondary" onClick={() => void exportLibrary()} disabled={busy === "export"}>
              Export
            </Button>
            <Button size="sm" onClick={() => void uploadFulltext()} disabled={!selected || busy === "upload"}>
              Upload full text
            </Button>
          </div>
        </div>

        <Tabs defaultValue="metadata" className="flex min-h-0 flex-1 flex-col">
          <TabsList className="mx-4 mt-3">
            <TabsTrigger value="metadata">Metadata</TabsTrigger>
            <TabsTrigger value="fulltext">Full text</TabsTrigger>
            <TabsTrigger value="notes">Notes</TabsTrigger>
          </TabsList>
          {error && (
            <div className="mx-4 mt-3 rounded-md border border-destructive/40 bg-destructive/10 p-2 text-sm text-destructive">
              {error}
            </div>
          )}
          <TabsContent value="metadata" className="min-h-0 flex-1 overflow-auto p-4">
            {!selected ? (
              <div className="grid h-full place-items-center text-sm text-muted-foreground">
                Select an article
              </div>
            ) : (
              <MetadataTable>
                {(selected.authors ?? []).map((author, index) => (
                  <MetadataRow
                    key={`${author.name ?? author.last_name ?? author.fore_name ?? "author"}-${index}`}
                    label="Author"
                    value={authorName(author)}
                  />
                ))}
                <MetadataRow label="Title" value={selected.title} />
                <MetadataRow label="Publication" value={selected.journal} />
                <MetadataRow label="Date" value={selected.year} />

                {identifierRows.map(({ kind, label }) => {
                  const value = articleIdentifier(selected, kind);
                  return (
                    <MetadataRow
                      key={kind}
                      label={label}
                      value={value}
                      href={value ? identifierHref(kind, value) : null}
                    />
                  );
                })}

                {selected.keywords?.length ? (
                  <MetadataRow label="Keywords">
                    <div className="flex flex-wrap gap-1 pt-px">
                      {selected.keywords.map((keyword) => (
                        <Badge key={keyword} variant="outline">{keyword}</Badge>
                      ))}
                    </div>
                  </MetadataRow>
                ) : null}

                <MetadataRow label="Source" value={selected.source} />
                <MetadataRow label="Record" value={selected.id} />
              </MetadataTable>
            )}
          </TabsContent>
          <TabsContent value="fulltext" className="min-h-0 flex-1 overflow-auto p-4">
            {detailLoading ? "Loading…" : activeDetail?.fulltext?.text_content ? (
              <article className="text-[13px] leading-6 break-words whitespace-pre-wrap">
                {activeDetail.fulltext.text_content}
              </article>
            ) : (
              <div className="grid h-full place-items-center text-sm text-muted-foreground">
                {activeDetail?.fulltext?.extract_status
                  ? `Extraction: ${activeDetail.fulltext.extract_status}`
                  : "No full text"}
              </div>
            )}
          </TabsContent>
          <TabsContent value="notes" className="min-h-0 flex-1 overflow-auto p-4">
            {detailLoading ? "Loading…" : activeDetail?.annotations.length ? (
              <div className="space-y-3">
                {activeDetail.annotations.map((note) => (
                  <article key={note.id} className="space-y-1">
                    <Badge variant="outline">{note.kind ?? "note"}</Badge>
                    <p className="text-[13px] leading-6 whitespace-pre-wrap">{note.content}</p>
                  </article>
                ))}
              </div>
            ) : (
              <div className="grid h-full place-items-center text-sm text-muted-foreground">
                No notes yet
              </div>
            )}
          </TabsContent>
        </Tabs>
      </Card>
    </div>
  );
}
