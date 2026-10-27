import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  BookOpen,
  ChevronDown,
  ChevronRight,
  Download,
  ExternalLink,
  Camera,
  File,
  FileQuestion,
  FileText,
  FileX2,
  Link2,
  NotebookText,
  Plus,
  PanelLeft,
  PanelRight,
  Search,
  StickyNote,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { gatewayRequest, pickUploadFile, saveTextFile, uploadBibFulltext, watchBibCollections } from "@/gateway";
import { CollectionTree, type CollectionAction } from "@/collection-tree";
import { collectionPath } from "@/collection-tree-model";
import { openPdfReader } from "@/pdf-reader";
import { openTxtReader } from "@/txt-reader";
import type { BibArticle, BibCollection, BibDetail } from "@/types";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Checkbox } from "@/components/ui/checkbox";
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
  kind: "pdf" | "text" | "link" | "notes";
  format?: string | null;
  href?: string;
  onClick?: () => void;
};

type AttachmentSummary = {
  pdf: boolean;
  snapshot: boolean;
  link: boolean;
  file: boolean;
  note: boolean;
  count: number;
};

const emptyAttachmentSummary: AttachmentSummary = {
  pdf: false,
  snapshot: false,
  link: false,
  file: false,
  note: false,
  count: 0,
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
    const format = fullText.file_format?.toLowerCase() ?? null;
    entries.push({
      key: "fulltext",
      kind: format === "txt" ? "text" : "pdf",
      format,
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


function AttachmentRow({ entry }: { entry: AttachmentEntry }) {
  const Icon =
    entry.kind === "pdf" ? FileText
    : entry.kind === "text" ? File
    : entry.kind === "link" ? Link2
    : NotebookText;
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

function AttachmentTypeIcons({ summary }: { summary: AttachmentSummary }) {
  const items = [
    { key: "pdf", show: summary.pdf, icon: FileText, label: "PDF attachment" },
    { key: "snapshot", show: summary.snapshot, icon: Camera, label: "HTML snapshot" },
    { key: "link", show: summary.link, icon: Link2, label: "Linked attachment" },
    { key: "file", show: summary.file, icon: File, label: "Stored file" },
    { key: "note", show: summary.note, icon: StickyNote, label: "Notes" },
  ].filter((item) => item.show);

  if (!items.length) {
    return (
      <FileQuestion
        aria-label="No attachments"
        className="size-3.5 text-muted-foreground/35"
      />
    );
  }

  return (
    <span className="flex items-center justify-center gap-0.5">
      {items.map(({ key, icon: Icon, label }) => (
        <Icon
          key={key}
          aria-label={label}
          className="size-3.5 text-muted-foreground"
        />
      ))}
    </span>
  );
}

export function BibliographyView() {
  const [collections, setCollections] = useState<BibCollection[]>([]);
  const [articles, setArticles] = useState<BibArticle[]>([]);
  const [attachmentSummaries, setAttachmentSummaries] = useState<Record<string, AttachmentSummary>>({});
  const [total, setTotal] = useState(0);
  const [queryDraft, setQueryDraft] = useState("");
  const [query, setQuery] = useState("");
  const [selectedCollection, setSelectedCollection] = useState<string | null>(null);
  const [includeDescendants, setIncludeDescendants] = useState(() => { try { return localStorage.getItem("autonomics.collections.recursive") === "true"; } catch { return false; } });
  const [associations, setAssociations] = useState<Array<{ collection_id: string; article_id: string }>>([]);
  const [libraryTotal, setLibraryTotal] = useState(0);
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
  /** Checked records in click order; drives selective BibTeX export. */
  const [checkedIds, setCheckedIds] = useState<string[]>([]);
  const requestSequence = useRef(0);

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
      const base = selectedCollection === "@unfiled" ? "/bib/articles/unfiled"
        : selectedCollection ? `/bib/collections/${encodeURIComponent(selectedCollection)}/articles` : "/bib/articles";
      const [collectionResponse, health] = await Promise.all([
        gatewayRequest("GET", "/bib/collections?include_deleted=true&view=summary"),
        gatewayRequest("GET", "/bib/health"),
      ]);
      if (requestId !== requestSequence.current) return;
      const nextCollections: BibCollection[] = collectionResponse.collections ?? [];
      if (selectedCollection && selectedCollection !== "@unfiled" && !nextCollections.some(c => c.id === selectedCollection && !c.deleted)) {
        setCollections(nextCollections);
        setSelectedCollection(null); setSelectedArticle(null); setDetail(null); setArticles([]);
        return;
      }
      const articleResponse = await gatewayRequest("GET", `${base}?query=${encodeURIComponent(nextQuery)}&include_descendants=${includeDescendants}&limit=500&offset=${offset}`);
      const nextArticles: BibArticle[] = articleResponse.articles ?? [];
      const nextSummaries = articleResponse.attachment_summaries ?? {};
      if (requestId !== requestSequence.current) return;
      setCollections(collectionResponse.collections ?? []);
      setLibraryTotal(health.article_count ?? articleResponse.total ?? 0);
      setAssociations(current => append ? [...current, ...(articleResponse.associations ?? [])] : articleResponse.associations ?? []);
      setArticles((current) => append ? [...current, ...nextArticles] : nextArticles);
      setAttachmentSummaries((current) => append
        ? { ...current, ...nextSummaries }
        : nextSummaries);
      setTotal(articleResponse.total ?? (articleResponse.articles ?? []).length);
      setArticleOffset(offset + nextArticles.length);
      setHasMoreArticles(offset + nextArticles.length < (articleResponse.total ?? 0));
    } catch (cause) {
      if (requestId !== requestSequence.current) return;
      setError(String(cause));
    } finally {
      if (requestId === requestSequence.current) setLoading(false);
    }
  }, [selectedCollection, includeDescendants]);

  useEffect(() => {
    void loadLibrary(query);
  }, [loadLibrary, query]);

  useEffect(() => {
    const refresh = () => void loadLibrary(query);
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, [loadLibrary, query]);

  useEffect(() => { try { localStorage.setItem("autonomics.collections.recursive", String(includeDescendants)); } catch { /* preference storage may be unavailable */ } }, [includeDescendants]);

  const latestReload=useRef<()=>void>(()=>{});
  latestReload.current=()=>void loadLibrary(query);
  useEffect(()=>watchBibCollections(()=>latestReload.current()),[]);

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

  const visibleArticles = articles;

  const toggleChecked = useCallback((articleId: string, checked: boolean) => {
    setCheckedIds((current) => checked
      ? current.includes(articleId) ? current : [...current, articleId]
      : current.filter((id) => id !== articleId));
  }, []);

  const visibleIds = useMemo(() => visibleArticles.map((article) => article.id), [visibleArticles]);
  const allVisibleChecked = visibleIds.length > 0 && visibleIds.every((id) => checkedIds.includes(id));
  const someVisibleChecked = visibleIds.some((id) => checkedIds.includes(id));
  const toggleAllChecked = (checked: boolean) => {
    setCheckedIds((current) => checked
      ? [...current, ...visibleIds.filter((id) => !current.includes(id))]
      : current.filter((id) => !visibleIds.includes(id)));
  };

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

  const selectCollection = (collectionId: string | null) => {
    if (collectionId === selectedCollection) return;
    requestSequence.current++;
    setSelectedCollection(collectionId);
    setArticles([]);
    setSelectedArticle(null);
    setDetail(null);
    setLibraryOpen(false);
  };

  const collectionAction = async (action: CollectionAction) => {
    const response = await gatewayRequest(action.method, action.path, action.body);
    if (response.deleted_ids?.includes(selectedCollection)) {
      const deleted = new Set<string>(response.deleted_ids);
      let parent = collections.find(c => c.id === selectedCollection)?.parent_id ?? null;
      while (parent && deleted.has(parent)) parent = collections.find(c => c.id === parent)?.parent_id ?? null;
      selectCollection(parent);
      return;
    } else if (action.method === "POST" && response.collection?.id) {
      if (response.collection.id !== selectedCollection) { selectCollection(response.collection.id); return; }
    }
    await loadLibrary(query);
  };

  const updateMembership = async (collectionId: string, articleId: string, remove = false) => {
    try {
      await gatewayRequest(remove ? "DELETE" : "POST", `/bib/collections/${encodeURIComponent(collectionId)}/articles${remove ? `/${encodeURIComponent(articleId)}` : ""}`, remove ? undefined : { article_id: articleId });
      await loadLibrary(query);
    } catch (cause) { setError(String(cause)); }
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
        collection_id: selectedCollection && selectedCollection !== "@unfiled" ? selectedCollection : null,
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
      const response = await gatewayRequest("GET", `/bib/export?format=bibtex&query=${encodeURIComponent(query)}${selectedCollection === "@unfiled" ? "&unfiled=true" : ""}${checkedIds.length ? `&ids=${encodeURIComponent(checkedIds.join(","))}` : ""}${!checkedIds.length && selectedCollection && selectedCollection !== "@unfiled" ? `&collection_id=${encodeURIComponent(selectedCollection)}&include_descendants=${includeDescendants}` : ""}`);
      await saveTextFile("autonomics.bib", response.export ?? "");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(null);
    }
  };

  const uploadFulltext = async () => {
    if (!selected) return;
    await uploadForArticle(selected);
  };

  const readerRef = useRef<{ close: () => void } | null>(null);
  const openReaderFor = (article: BibArticle, format?: string | null) => {
    readerRef.current?.close();
    readerRef.current = null;
    const openReader = format === "txt" ? openTxtReader : openPdfReader;
    void openReader({ articleId: article.id, title: article.title, article })
      .then((handle) => { readerRef.current = handle; })
      .catch((cause) => setError(String(cause)));
  };
  const readFulltext = (format: string | null | undefined) => {
    if (!selected) return;
    openReaderFor(selected, format);
  };
  const withReader = (entry: AttachmentEntry): AttachmentEntry =>
    entry.kind === "pdf" || entry.kind === "text"
      ? { ...entry, onClick: () => readFulltext(entry.format) }
      : entry;

  const patchSummary = (articleId: string, patch: (summary: AttachmentSummary) => AttachmentSummary) => {
    setAttachmentSummaries((current) => ({
      ...current,
      [articleId]: patch(current[articleId] ?? emptyAttachmentSummary),
    }));
  };

  const uploadForArticle = async (article: BibArticle) => {
    const file = await pickUploadFile();
    if (!file) return;
    setBusy("upload");
    setError(null);
    try {
      const response = await uploadBibFulltext(article.id, file);
      const format = String(response?.fulltext?.file_format ?? "").toLowerCase();
      patchSummary(article.id, (summary) => ({
        ...summary,
        pdf: summary.pdf || format === "pdf",
        file: true,
        count: Math.max(summary.count, 1),
      }));
      if (selectedArticle === article.id) selectArticle(article.id);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(null);
    }
  };

  const deleteFulltextFor = async (article: BibArticle) => {
    setBusy("delete-attachment");
    setError(null);
    try {
      await gatewayRequest("DELETE", `/bib/articles/${encodeURIComponent(article.id)}/fulltext`);
      patchSummary(article.id, (summary) => ({
        ...summary,
        pdf: false,
        file: false,
        count: Math.max(0, summary.count - 1),
      }));
      if (selectedArticle === article.id) selectArticle(article.id);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(null);
    }
  };

  const deleteArticleFor = async (article: BibArticle) => {
    if (!window.confirm(`Delete “${article.title}”? Its attachments and notes are removed too.`)) return;
    setBusy("delete-article");
    setError(null);
    readerRef.current?.close();
    readerRef.current = null;
    try {
      await gatewayRequest("DELETE", `/bib/articles/${encodeURIComponent(article.id)}`);
      if (selectedArticle === article.id) {
        setSelectedArticle(null);
        setDetail(null);
      }
      await loadLibrary(query);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(null);
    }
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
          <CollectionTree collections={collections} selected={selectedCollection} total={libraryTotal} onSelect={selectCollection} onAction={collectionAction} />
          <label className="mt-3 flex items-start gap-2 border-t px-2 py-3 text-xs text-muted-foreground"><input type="checkbox" checked={includeDescendants} onChange={e => setIncludeDescendants(e.target.checked)} />Show items from subcollections</label>
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

        <div className="grid h-8 shrink-0 grid-cols-[24px_24px_minmax(0,1fr)_56px] items-center border-b bg-muted/60 px-2 text-[11px] font-medium tracking-wide text-muted-foreground uppercase md:grid-cols-[24px_24px_minmax(0,1fr)_168px_80px]">
          <Checkbox
            aria-label="Select all visible articles"
            checked={allVisibleChecked}
            indeterminate={!allVisibleChecked && someVisibleChecked}
            onCheckedChange={(checked) => toggleAllChecked(checked === true)}
          />
          <span />
          <span>Title{checkedIds.length ? ` (${checkedIds.length} selected)` : ""}</span>
          <span className="hidden md:block">Creator</span>
          <span className="text-center">Files</span>
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
            const summary = article.id === selected?.id
              ? {
                  ...emptyAttachmentSummary,
                  ...attachmentSummaries[article.id],
                  pdf: attachmentSummaries[article.id]?.pdf || Boolean(activeDetail?.fulltext),
                  note: attachmentSummaries[article.id]?.note || Boolean(activeDetail?.annotations.length),
                  count: attachmentSummaries[article.id]?.count ?? (activeDetail ? attachments.length : 0),
                }
              : attachmentSummaries[article.id] ?? emptyAttachmentSummary;

            return (
              <ContextMenu key={article.id}>
                <ContextMenuTrigger render={<div className="border-b border-border/60" />}>
                  <div
                    className={cn(
                      "grid grid-cols-[24px_24px_minmax(0,1fr)_56px] items-center md:grid-cols-[24px_24px_minmax(0,1fr)_168px_80px]",
                      (isSelected || checkedIds.includes(article.id)) && "bg-accent",
                    )}
                  >
                    <span className="flex h-9 items-center justify-center">
                      <Checkbox
                        aria-label={`Select ${article.title}`}
                        checked={checkedIds.includes(article.id)}
                        onCheckedChange={(checked) => toggleChecked(article.id, checked === true)}
                      />
                    </span>
                    <button
                      type="button"
                      draggable
                      onDragStart={event => { event.dataTransfer.setData("application/x-autonomics-article", article.id); event.dataTransfer.effectAllowed = "copy"; }}
                      aria-expanded={isSelected && attachmentsOpen}
                      onClick={() => isSelected ? setAttachmentsOpen(!attachmentsOpen) : selectArticle(article.id)}
                      className={cn(
                        "col-span-3 grid h-9 w-full grid-cols-[24px_minmax(0,1fr)_56px] items-center px-2 text-left transition-colors hover:bg-accent/70 md:col-span-4 md:grid-cols-[24px_minmax(0,1fr)_168px_80px]",
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
                        <AttachmentTypeIcons summary={summary} />
                      </span>
                    </button>
                  </div>

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
                          <AttachmentRow entry={withReader(entry)} />
                        </div>
                      ))}
                    </div>
                  )}
                </ContextMenuTrigger>
                <ContextMenuContent>
                  <ContextMenuItem
                    disabled={!summary.pdf}
                    onClick={() => openReaderFor(article)}
                  >
                    <BookOpen />
                    Read
                  </ContextMenuItem>
                  <ContextMenuItem disabled={busy === "upload"} onClick={() => void uploadForArticle(article)}>
                    <Upload />
                    Upload attachment
                  </ContextMenuItem>
                  <ContextMenuItem
                    variant="destructive"
                    disabled={!summary.pdf || busy === "delete-attachment"}
                    onClick={() => void deleteFulltextFor(article)}
                  >
                    <FileX2 />
                    Delete attachment
                  </ContextMenuItem>
                  <ContextMenuSeparator />
                  <ContextMenuItem
                    variant="destructive"
                    disabled={busy === "delete-article"}
                    onClick={() => void deleteArticleFor(article)}
                  >
                    <Trash2 />
                    Delete article
                  </ContextMenuItem>
                </ContextMenuContent>
              </ContextMenu>
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
                      <AttachmentRow key={entry.key} entry={withReader(entry)} />
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

              <Section title="Collections" count={associations.filter(a => a.article_id === selected.id).length}>
                <div className="space-y-2">
                  {associations.filter(a => a.article_id === selected.id).map(a => <div key={a.collection_id} className="flex items-center gap-1 text-xs"><button className="min-w-0 flex-1 truncate text-left hover:underline" onClick={() => selectCollection(a.collection_id)}>{collectionPath(collections, a.collection_id)}</button><Button size="icon-xs" variant="ghost" aria-label={`Remove from ${collectionPath(collections, a.collection_id)}`} onClick={() => void updateMembership(a.collection_id, selected.id, true)}><X /></Button></div>)}
                  <select aria-label="Add article to collection" value="" className="w-full rounded border bg-background p-1.5 text-xs" onChange={e => { if (e.target.value) void updateMembership(e.target.value, selected.id); }}><option value="">Add to collection…</option>{collections.filter(c => !c.deleted && !associations.some(a => a.article_id === selected.id && a.collection_id === c.id)).map(c => <option key={c.id} value={c.id}>{collectionPath(collections, c.id)}</option>)}</select>
                </div>
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
            <Button
              size="xs"
              variant="ghost"
              onClick={() => readFulltext(activeDetail?.fulltext?.file_format)}
              disabled={!activeDetail?.fulltext}
            >
              <FileText />
              {activeDetail?.fulltext?.file_format?.toLowerCase() === "txt" ? "Read TXT" : "Read PDF"}
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
