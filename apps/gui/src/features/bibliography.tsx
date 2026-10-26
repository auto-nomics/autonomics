import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  ChevronDown,
  ChevronRight,
  Download,
  ExternalLink,
  FileText,
  Folder,
  Link2,
  NotebookText,
  Plus,
  PanelLeft,
  PanelRight,
  Search,
  StickyNote,
  Upload,
  X,
} from "lucide-react";
import { gatewayRequest, pickUploadFile, saveTextFile, uploadBibFulltext } from "@/gateway";
import { openPdfReader } from "@/pdf-reader";
import type { BibArticle, BibCollection, BibDetail } from "@/types";
import { Badge } from "@/components/ui/badge";
import { Button, buttonVariants } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";

type BibAuthor = NonNullable<BibArticle["authors"]>[number];

type MetadataRowProps = {
  label: string;
  value?: string | number | null;
  href?: string | null;
  children?: ReactNode;
};

type AttachmentEntry = {
  key: string;
  label: string;
  caption: string;
  kind: "pdf" | "link" | "notes";
  href?: string;
  onClick?: () => void;
};

function authorName(author: BibAuthor) {
  if (author.name) return author.name;
  return [author.last_name, author.fore_name].filter(Boolean).join(", ");
}

function creatorDisplay(article: BibArticle) {
  const authors = article.authors ?? [];
  if (!authors.length) return article.source || article.journal || "Unknown";
  if (authors.length === 1) return authorName(authors[0]);
  const first = authors[0];
  const surname = first.last_name || first.name?.split(/\s+/).slice(-1)[0] || first.name;
  return surname ? `${surname} et al.` : "Unknown";
}

function creatorsText(article: BibArticle) {
  return (article.authors ?? []).map(authorName).filter(Boolean).join("; ");
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
    return /^PMC\d+$/.test(pmcid) ? `https://pmc.ncbi.nlm.nih.gov/articles/${pmcid}/` : null;
  }
  if (normalizedKind === "arxiv") {
    return `https://arxiv.org/abs/${value.replace(/^arxiv:/i, "")}`;
  }
  if (normalizedKind === "openalex" && /^[Ww]\d+$/.test(value)) {
    return `https://openalex.org/${value.toUpperCase()}`;
  }
  return null;
}

function formatFileSize(size: number | null | undefined) {
  if (!size) return null;
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(0)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

function attachmentFileName(path?: string | null) {
  if (!path) return "Full text";
  const normalized = path.split(/[\\/]/).filter(Boolean).pop();
  return normalized ? decodeURIComponent(normalized) : "Full text";
}

function attachmentEntries(article: BibArticle, detail: BibDetail | null): AttachmentEntry[] {
  const entries: AttachmentEntry[] = [];
  const fullText = detail?.fulltext;

  if (fullText) {
    entries.push({
      key: "fulltext",
      kind: "pdf",
      label: attachmentFileName(fullText.file_path),
      caption: [
        fullText.file_format?.toUpperCase() || "File",
        formatFileSize(fullText.file_size),
        fullText.extract_status === "done" ? "Indexed" : fullText.extract_status || null,
      ].filter(Boolean).join(" · "),
    });
  }

  const identifierKinds = ["pmid", "doi", "pmc", "arxiv", "openalex"];
  for (const kind of identifierKinds) {
    const value = articleIdentifier(article, kind);
    const href = value ? identifierHref(kind, value) : null;
    if (!value || !href) continue;
    entries.push({
      key: `identifier-${kind}`,
      kind: "link",
      label: `${kind === "pmid" ? "PubMed" : kind} entry`,
      caption: value,
      href,
    });
    break;
  }

  if (detail?.annotations.length) {
    entries.push({
      key: "notes",
      kind: "notes",
      label: "Notes",
      caption: `${detail.annotations.length} item${detail.annotations.length === 1 ? "" : "s"}`,
    });
  }

  return entries;
}

function MetadataTable({ children }: { children: ReactNode }) {
  return (
    <div className="grid grid-cols-[84px_minmax(0,1fr)] gap-x-3 gap-y-0.5">
      {children}
    </div>
  );
}

function MetadataRow({ label, value, href, children }: MetadataRowProps) {
  if (!value && !children) return null;

  return (
    <div className="group/row col-span-2 grid grid-cols-[subgrid]">
      <div className="pt-1 text-right text-xs leading-5 text-muted-foreground">{label}</div>
      <div className="flex min-w-0 items-start gap-1">
        <div
          className="min-w-0 flex-1 rounded-[5px] px-1 py-[3px] text-[13px] leading-5 break-words transition-colors group-hover/row:bg-accent/40"
          aria-label={label}
        >
          {children ?? <span className="break-words whitespace-pre-wrap">{value}</span>}
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

function Section({
  title,
  count,
  children,
}: {
  title: string;
  count?: number | null;
  children: ReactNode;
}) {
  return (
    <section className="border-b border-border/70 pb-3 last:border-b-0">
      <div className="flex h-8 items-center gap-1.5 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">
        {title}
        {count != null && <span className="text-[11px] opacity-70">{count}</span>}
      </div>
      {children}
    </section>
  );
}

function SidebarItem({
  label,
  count,
  active,
  onClick,
}: {
  label: string;
  count?: number;
  active?: boolean;
  onClick?: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "flex h-7 w-full items-center gap-2 rounded-md px-2 text-left text-[13px] transition-colors hover:bg-accent",
        active && "bg-accent font-medium text-foreground",
      )}
    >
      <Folder className="size-3.5 shrink-0 text-muted-foreground" />
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {count != null && (
        <span className="shrink-0 text-xs text-muted-foreground">{count.toLocaleString()}</span>
      )}
    </button>
  );
}

function AttachmentRow({ entry }: { entry: AttachmentEntry }) {
  const Icon = entry.kind === "pdf" ? FileText : entry.kind === "link" ? Link2 : NotebookText;
  const content = (
    <>
      <Icon className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[13px]">{entry.label}</span>
        <span className="block truncate text-xs text-muted-foreground">{entry.caption}</span>
      </span>
    </>
  );
  const className = "flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-accent";

  if (entry.href) {
    return (
      <a href={entry.href} target="_blank" rel="noreferrer" className={className}>
        {content}
      </a>
    );
  }

  return (
    <button type="button" onClick={entry.onClick} className={className} disabled={!entry.onClick}>
      {content}
    </button>
  );
}

export function BibliographyView() {
  const [collections, setCollections] = useState<BibCollection[]>([]);
  const [articles, setArticles] = useState<BibArticle[]>([]);
  const [total, setTotal] = useState(0);
  const [queryDraft, setQueryDraft] = useState("");
  const [query, setQuery] = useState("");
  const [selectedCollection, setSelectedCollection] = useState<string | null>(null);
  const [collectionArticles, setCollectionArticles] = useState<BibArticle[]>([]);
  const [selectedArticle, setSelectedArticle] = useState<string | null>(null);
  const [attachmentsOpen, setAttachmentsOpen] = useState(true);
  const [detail, setDetail] = useState<BibDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [libraryOpen, setLibraryOpen] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [articleOffset, setArticleOffset] = useState(0);
  const [hasMoreArticles, setHasMoreArticles] = useState(false);
  const requestSequence = useRef(0);
  const collectionRequestSequence = useRef(0);
  const articleLoaderRef = useRef<HTMLDivElement | null>(null);

  const loadLibrary = useCallback(async (
    nextQuery = "",
    options: { append?: boolean; offset?: number } = {},
  ) => {
    const append = options.append ?? false;
    const offset = options.offset ?? 0;
    const requestId = ++requestSequence.current;
    setLoading(true);
    setError(null);
    try {
      const [collectionResponse, articleResponse] = await Promise.all([
        gatewayRequest("GET", "/bib/collections"),
        gatewayRequest("GET", `/bib/articles?query=${encodeURIComponent(nextQuery)}&limit=500&offset=${offset}`),
      ]);
      const nextArticles: BibArticle[] = articleResponse.articles ?? [];
      if (requestId !== requestSequence.current) return;
      setCollections(collectionResponse.collections ?? []);
      setArticles((current) => append ? [...current, ...nextArticles] : nextArticles);
      setTotal(articleResponse.total ?? (articleResponse.articles ?? []).length);
      setArticleOffset(offset + nextArticles.length);
      setHasMoreArticles(nextArticles.length === 500);
    } catch (cause) {
      if (requestId !== requestSequence.current) return;
      setError(String(cause));
    } finally {
      if (requestId === requestSequence.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadLibrary("");
  }, [loadLibrary]);

  const loadMoreArticles = useCallback(() => {
    if (loading || !hasMoreArticles) return;
    void loadLibrary(query, { append: true, offset: articleOffset });
  }, [articleOffset, hasMoreArticles, loadLibrary, loading, query]);

  useEffect(() => {
    const node = articleLoaderRef.current;
    if (!node || !hasMoreArticles || loading) return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) loadMoreArticles();
    }, { rootMargin: "400px" });
    observer.observe(node);
    return () => observer.disconnect();
  }, [hasMoreArticles, loadMoreArticles, loading]);

  const visibleArticles = useMemo(() => {
    const source = selectedCollection ? collectionArticles : articles;
    const needle = query.trim().toLowerCase();
    if (!needle) return source;
    return source.filter((article) => {
      const authors = (article.authors ?? [])
        .map((author) => author.name || [author.last_name, author.fore_name].filter(Boolean).join(" "))
        .join(" ");
      return `${article.title} ${authors} ${article.journal ?? ""} ${article.id}`
        .toLowerCase()
        .includes(needle);
    });
  }, [articles, collectionArticles, query, selectedCollection]);

  const selectArticle = useCallback((articleId: string | null) => {
    setSelectedArticle(articleId);
    setAttachmentsOpen(true);
    setDetail(null);
    if (!articleId) return;
    if (!window.matchMedia("(min-width: 1280px)").matches) setDetailsOpen(true);
    setDetailLoading(true);
    void gatewayRequest("GET", `/bib/articles/${encodeURIComponent(articleId)}`)
      .then((response) => setDetail(response as BibDetail))
      .catch((cause) => setError(String(cause)))
      .finally(() => setDetailLoading(false));
  }, []);

  const loadCollectionArticles = useCallback(async (collectionId: string) => {
    const requestId = ++collectionRequestSequence.current;
    setLoading(true);
    setError(null);
    try {
      const response = await gatewayRequest(
        "GET",
        `/bib/collections/${encodeURIComponent(collectionId)}/articles`,
      );
      if (requestId !== collectionRequestSequence.current) return;
      setCollectionArticles(response.articles ?? []);
    } catch (cause) {
      if (requestId !== collectionRequestSequence.current) return;
      setError(String(cause));
    } finally {
      if (requestId === collectionRequestSequence.current) setLoading(false);
    }
  }, []);

  const selectCollection = (collectionId: string | null) => {
    setSelectedCollection(collectionId);
    setSelectedArticle(null);
    setDetail(null);
    setLibraryOpen(false);
    if (collectionId) void loadCollectionArticles(collectionId);
  };

  const selected = visibleArticles.find((article) => article.id === selectedArticle)
    ?? detail?.article
    ?? null;
  const activeDetail = selected && detail?.article.id === selected.id ? detail : null;
  const attachments = selected ? attachmentEntries(selected, activeDetail) : [];
  const identifierRows = [
    { kind: "doi", label: "DOI" },
    { kind: "pmid", label: "PMID" },
    { kind: "pmc", label: "PMCID" },
    { kind: "arxiv", label: "arXiv" },
    { kind: "openalex", label: "OpenAlex" },
  ];

  const importArticle = async () => {
    const identifier = window.prompt("DOI, PMID, arXiv, or OpenAlex ID");
    if (!identifier) return;
    setBusy("import");
    setError(null);
    try {
      const response = await gatewayRequest("POST", "/bib/articles/import", {
        id_type: "doi",
        id: identifier,
        source: null,
        fetch_fulltext: true,
      });
      setQueryDraft("");
      setQuery("");
      await loadLibrary("");
      if (response.article?.id) selectArticle(response.article.id);
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
    void openPdfReader({
      articleId: selected.id,
      title: selected.title,
      article: selected,
    }).catch((cause) => setError(String(cause)));
  };

  return (
    <div className="grid min-h-0 flex-1 grid-rows-[minmax(0,1fr)] gap-2 xl:grid-cols-[216px_minmax(0,1fr)_336px] xl:grid-rows-1">
      {(libraryOpen || detailsOpen) && (
        <button
          type="button"
          aria-label="Close panel"
          onClick={() => {
            setLibraryOpen(false);
            setDetailsOpen(false);
          }}
          className="fixed inset-0 z-30 bg-black/50 outline-none xl:hidden"
        />
      )}
      <aside
        className={cn(
          "relative flex min-h-0 flex-col overflow-hidden rounded-lg border bg-card",
          "max-xl:fixed max-xl:top-13 max-xl:bottom-0 max-xl:left-0 max-xl:z-40 max-xl:w-[min(88vw,300px)] max-xl:rounded-none max-xl:shadow-2xl max-xl:transition-transform",
          libraryOpen ? "max-xl:translate-x-0" : "max-xl:-translate-x-full",
        )}
      >
        <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b px-3">
          <span className="text-xs font-semibold tracking-wide uppercase">Library</span>
          <div className="flex shrink-0 items-center gap-1">
            <Badge variant="secondary">{total.toLocaleString()}</Badge>
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label="Close library"
              className="xl:hidden"
              onClick={() => setLibraryOpen(false)}
            >
              <X />
            </Button>
          </div>
        </div>
        <ScrollArea className="min-h-0 flex-1" contentClassName="p-1.5">
          <SidebarItem
            label="All Articles"
            count={total}
            active={!selectedCollection}
            onClick={() => selectCollection(null)}
          />
          {collections.map((collection) => (
            <SidebarItem
              key={collection.id}
              label={collection.name}
              count={collection.article_ids.length}
              active={selectedCollection === collection.id}
              onClick={() => selectCollection(collection.id)}
            />
          ))}
        </ScrollArea>
      </aside>

      <section className="flex min-h-0 flex-col overflow-hidden rounded-lg border bg-card">
        <div className="flex h-10 shrink-0 items-center gap-2 border-b px-2">
          <Button
            size="icon-sm"
            variant="ghost"
            aria-label="Open library"
            className="xl:hidden"
            onClick={() => setLibraryOpen(true)}
          >
            <PanelLeft />
          </Button>
          <form
            className="relative min-w-0 flex-1"
            onSubmit={(event) => {
              event.preventDefault();
                setQuery(queryDraft);
                void loadLibrary(queryDraft);
            }}
          >
            <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={queryDraft}
              placeholder={`Search ${total.toLocaleString()} items`}
              className="h-7 pl-8 text-[13px]"
              onChange={(event) => setQueryDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  setQuery(queryDraft);
                  void loadLibrary(queryDraft);
                }
              }}
            />
          </form>
          <Button size="icon-sm" variant="ghost" aria-label="Import article" title="Import article" onClick={() => void importArticle()} disabled={busy === "import"}>
            <Plus />
          </Button>
          <Button
            size="icon-sm"
            variant="ghost"
            aria-label="Open entry details"
            className="xl:hidden"
            onClick={() => setDetailsOpen(true)}
          >
            <PanelRight />
          </Button>
          <Button size="icon-sm" variant="ghost" aria-label="Upload full text" title="Upload full text" onClick={() => void uploadFulltext()} disabled={!selected || busy === "upload"}>
            <Upload />
          </Button>
          <Button size="icon-sm" variant="ghost" aria-label="Export BibTeX" title="Export BibTeX" onClick={() => void exportLibrary()} disabled={busy === "export"}>
            <Download />
          </Button>
        </div>

        <div className="grid h-8 shrink-0 grid-cols-[24px_minmax(0,1fr)_30px] items-center border-b bg-muted/60 px-2 text-[11px] font-medium tracking-wide text-muted-foreground uppercase md:grid-cols-[28px_minmax(0,1fr)_168px_34px]">
          <span />
          <span>Title</span>
          <span className="hidden md:block">Creator</span>
          <span className="text-center">#</span>
        </div>

        <ScrollArea className="min-h-0 flex-1" contentClassName="pb-6">
          {loading && (
            <div className="space-y-px">
              {Array.from({ length: 10 }).map((_, index) => (
                <div key={index} className="h-9 animate-pulse border-b bg-muted/30" />
              ))}
            </div>
          )}

          {!loading && visibleArticles.map((article) => {
            const isSelected = article.id === selectedArticle;
            const hasFullText = isSelected && Boolean(activeDetail?.fulltext);

            return (
              <div key={article.id} className="border-b border-border/60">
                <button
                  type="button"
	                  aria-expanded={isSelected && attachmentsOpen}
	                  onClick={() => isSelected ? setAttachmentsOpen(!attachmentsOpen) : selectArticle(article.id)}
                  className={cn(
                    "grid h-9 w-full grid-cols-[24px_minmax(0,1fr)_30px] items-center px-2 text-left transition-colors hover:bg-accent/70 md:grid-cols-[28px_minmax(0,1fr)_168px_34px]",
                    isSelected && "bg-accent",
                  )}
                >
                  <span className="flex items-center justify-center">
	                    {isSelected && attachmentsOpen ? (
                      <ChevronDown className="size-3.5 text-muted-foreground" />
                    ) : (
                      <ChevronRight className="size-3.5 text-muted-foreground/60" />
                    )}
                  </span>
                  <span className="min-w-0 truncate pr-3 text-[13px]">{article.title}</span>
                  <span className="hidden min-w-0 truncate pr-3 text-[13px] text-muted-foreground md:block">
                    {creatorDisplay(article)}
                  </span>
                  <span className="flex items-center justify-center text-muted-foreground">
                    <FileText className={cn("size-3.5", hasFullText ? "opacity-100" : "opacity-35")} />
                  </span>
                </button>

                {isSelected && attachmentsOpen && (
                  <div className="bg-muted/25">
                    {detailLoading && (
                      <div className="px-7 py-2 text-xs text-muted-foreground">Loading attachments…</div>
                    )}
                    {!detailLoading && !attachments.length && (
                      <div className="px-7 py-2 text-xs text-muted-foreground">No attachments</div>
                    )}
                    {!detailLoading && attachments.map((entry) => (
                      <div key={entry.key} className="pl-7 pr-2">
                        <AttachmentRow entry={entry.kind === "pdf" ? { ...entry, onClick: readPdf } : entry} />
                      </div>
                    ))}
                  </div>
                )}
              </div>
            );
          })}

          {!loading && !visibleArticles.length && (
            <div className="grid h-full place-items-center text-sm text-muted-foreground">No matching articles</div>
          )}
          {!loading && hasMoreArticles && (
            <div ref={articleLoaderRef} className="p-3 text-center">
              <Button size="sm" variant="secondary" onClick={loadMoreArticles}>
                Load more ({articleOffset.toLocaleString()} / {total.toLocaleString()})
              </Button>
            </div>
          )}
        </ScrollArea>
      </section>

      <aside
        className={cn(
          "relative flex min-h-0 flex-col overflow-hidden rounded-lg border bg-card",
          "max-xl:fixed max-xl:top-13 max-xl:bottom-0 max-xl:right-0 max-xl:z-40 max-xl:w-[min(94vw,360px)] max-xl:rounded-none max-xl:shadow-2xl max-xl:transition-transform",
          detailsOpen ? "max-xl:translate-x-0" : "max-xl:translate-x-full",
        )}
      >
        <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b px-3">
          <span className="min-w-0 truncate text-[13px] font-medium">
            {selected ? "Entry details" : "Bibliography"}
          </span>
          <div className="flex shrink-0 items-center gap-1">
            {detailLoading && <Badge variant="secondary">Loading</Badge>}
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label="Close entry details"
              className="xl:hidden"
              onClick={() => setDetailsOpen(false)}
            >
              <X />
            </Button>
          </div>
        </div>

        <ScrollArea className="min-h-0 flex-1" contentClassName="p-3">
          {error && (
            <div className="mb-3 rounded-md border border-destructive/40 bg-destructive/10 p-2 text-xs text-destructive">
              {error}
            </div>
          )}

          {!selected ? (
            <div className="grid h-full place-items-center text-sm text-muted-foreground">Select an item</div>
          ) : (
            <div className="space-y-3">
              <Section title="Attachment Info" count={attachments.length}>
                {!detailLoading && attachments.length ? (
                  <div className="space-y-0.5">
                    {attachments.map((entry) => (
                      <AttachmentRow key={entry.key} entry={entry.kind === "pdf" ? { ...entry, onClick: readPdf } : entry} />
                    ))}
                  </div>
                ) : (
                  <div className="text-xs text-muted-foreground">
                    {detailLoading ? "Loading…" : "No attachment"}
                  </div>
                )}
              </Section>

              <Section title="Details">
                <MetadataTable>
                  <MetadataRow label="Title" value={selected.title} />
                  {(selected.authors ?? []).map((author, index) => (
                    <MetadataRow
                      key={`${author.name ?? author.last_name ?? author.fore_name ?? "author"}-${index}`}
                      label={index === 0 ? "Authors" : ""}
                      value={authorName(author)}
                    />
                  ))}
                  <MetadataRow label="Publication" value={selected.journal} />
                  <MetadataRow label="Date" value={selected.year} />
                  <MetadataRow label="Source" value={selected.source} />
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
                  <MetadataRow label="Abstract" value={selected.abstract_text} />
                  <MetadataRow label="Record" value={selected.id} />
                </MetadataTable>
              </Section>

              <Section title="Tags" count={selected.keywords?.length ?? 0}>
                {selected.keywords?.length ? (
                  <div className="flex flex-wrap gap-1">
                    {selected.keywords.map((keyword) => (
                      <Badge key={keyword} variant="outline" className="max-w-full">
                        <span className="truncate">{keyword}</span>
                      </Badge>
                    ))}
                  </div>
                ) : (
                  <div className="text-xs text-muted-foreground">No tags</div>
                )}
              </Section>

              <Section title="Notes" count={activeDetail?.annotations.length ?? 0}>
                {activeDetail?.annotations.length ? (
                  <div className="space-y-2">
                    {activeDetail.annotations.map((note) => (
                      <article key={note.id} className="rounded-md border p-2">
                        <div className="mb-1 flex items-center gap-1.5 text-xs text-muted-foreground">
                          <StickyNote className="size-3" />
                          {note.kind || "Note"}
                        </div>
                        <p className="text-[13px] leading-5 break-words whitespace-pre-wrap">{note.content}</p>
                      </article>
                    ))}
                  </div>
                ) : (
                  <div className="text-xs text-muted-foreground">No notes</div>
                )}
              </Section>
            </div>
          )}
        </ScrollArea>

        {selected && (
          <div className="flex h-10 shrink-0 items-center justify-between gap-2 border-t px-2">
            <Button size="xs" variant="ghost" onClick={readPdf} disabled={!activeDetail?.fulltext}>
              <FileText />
              Read PDF
            </Button>
            <span className="truncate text-xs text-muted-foreground">
              {creatorsText(selected) || selected.source || ""}
            </span>
          </div>
        )}
      </aside>
    </div>
  );
}
