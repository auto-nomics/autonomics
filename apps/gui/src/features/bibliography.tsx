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
import { SearchAssist, ColumnsPicker, readResultColumns, useQueryHistory } from "./search-assist";
import { readSearchLocation, writeSearchLocation } from "./search-location";
import { VirtualArticleList } from "./virtual-article-list";
import { AdvancedSearchBuilder } from "./advanced-search-builder";
import { SavedSearches } from "./saved-searches";
import { SearchFiltersPanel } from "./search-filters";
import { SearchEvidencePanel } from "./search-evidence";
import { SearchIndexStatus } from "./search-index-status";
import { useBibSearch } from "./use-bib-search";
import { SearchSnippet } from "./search-snippet";
import { SemanticSearchControls, RecallLabel } from "./semantic-search";
import { DiscoveryView } from "./discovery-view";
import { LiteratureExploration } from "./literature-exploration";
import { Subscriptions } from "./subscriptions";
import { SaveSnapshotButton, SearchSnapshots } from "./search-snapshots";
import { DiscoveryProvenance } from "./discovery-provenance";
import { hasFilters, formatExpression, quickFields, type SearchEvidence, type SearchField, type SearchExpr, type SearchRequest, type SavedSearch, type SearchFilters, type SearchFacets, type SearchHit, type SearchSort, type SearchMode, type SearchRetrieval } from "./bib-search-model";

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
  attachmentKey?: string | null;
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

  if (detail?.attachments?.length) {
    for (const attachment of detail.attachments) {
      const format = /pdf/i.test(attachment.content_type ?? "") || /\.pdf$/i.test(attachment.original_filename ?? "") ? "pdf" : "txt";
      if (attachment.link_mode === 3 && attachment.source_url) entries.push({key:attachment.key,kind:"link",label:attachment.title || "Linked file",caption:attachment.source_url,href:attachment.source_url});
      else entries.push({key:attachment.key,attachmentKey:attachment.key,kind:format === "pdf" ? "pdf" : "text",format,label:attachment.title || attachment.original_filename || "Attachment",caption:[format.toUpperCase(),formatFileSize(attachment.file_size)].filter(Boolean).join(" · ")});
    }
  } else if (fullText) {
    const format = fullText.file_format?.toLowerCase() ?? null;
    entries.push({
      key: "fulltext",
      attachmentKey: fullText.attachment_key,
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
  const [workspace, setWorkspace] = useState<"library" | "discover" | "snapshots" | "subscriptions">("library");
  const [discoveryMounted, setDiscoveryMounted] = useState(false);
  const [snapshotsMounted, setSnapshotsMounted] = useState(false);
  const [subscriptionsMounted, setSubscriptionsMounted] = useState(false);
  return <div className="flex min-h-0 flex-1 flex-col gap-2">
    <nav aria-label="Bibliography workspace" className="flex flex-wrap gap-1">
      <Button size="sm" variant={workspace === "library" ? "secondary" : "ghost"} aria-current={workspace === "library" ? "page" : undefined} onClick={() => setWorkspace("library")}>Library</Button>
      <Button size="sm" variant={workspace === "discover" ? "secondary" : "ghost"} aria-current={workspace === "discover" ? "page" : undefined} onClick={() => { setDiscoveryMounted(true); setWorkspace("discover"); }}>Discover</Button>
      <Button size="sm" variant={workspace === "snapshots" ? "secondary" : "ghost"} aria-current={workspace === "snapshots" ? "page" : undefined} onClick={() => { setSnapshotsMounted(true); setWorkspace("snapshots"); }}>Snapshots</Button>
      <Button size="sm" variant={workspace === "subscriptions" ? "secondary" : "ghost"} aria-current={workspace === "subscriptions" ? "page" : undefined} onClick={() => { setSubscriptionsMounted(true); setWorkspace("subscriptions"); }}>Subscriptions</Button>
    </nav>
    <div hidden={workspace !== "library"} className={workspace === "library" ? "flex min-h-0 flex-1 flex-col" : "hidden"}><LibraryView /></div>
    {discoveryMounted && <div hidden={workspace !== "discover"} className={workspace === "discover" ? "flex min-h-0 flex-1 flex-col" : "hidden"}><DiscoveryView /></div>}
    {snapshotsMounted && <div hidden={workspace !== "snapshots"} className={workspace === "snapshots" ? "flex min-h-0 flex-1 flex-col" : "hidden"}><SearchSnapshots active={workspace === "snapshots"} /></div>}
    {subscriptionsMounted && <div hidden={workspace !== "subscriptions"} className={workspace === "subscriptions" ? "flex min-h-0 flex-1 flex-col" : "hidden"}><Subscriptions active={workspace === "subscriptions"} /></div>}
  </div>;
}

function LibraryView() {
  const [initial] = useState(readSearchLocation);
  const keywordHistory=useQueryHistory();
  const topicHistory=useQueryHistory("autonomics.bib.topicHistory.v1");
  const [columns,setColumns]=useState(readResultColumns);
  const [showSnippets,setShowSnippets]=useState(()=>{try{return localStorage.getItem("autonomics.bib.snippets")!=="false";}catch{return true;}});
  const [wideColumns,setWideColumns]=useState(()=>window.matchMedia("(min-width:768px)").matches);
  useEffect(()=>{const media=window.matchMedia("(min-width:768px)");const update=()=>setWideColumns(media.matches);media.addEventListener("change",update);return()=>media.removeEventListener("change",update);},[]);
  useEffect(()=>{try{localStorage.setItem("autonomics.bib.columns.v1",JSON.stringify(columns));localStorage.setItem("autonomics.bib.snippets",String(showSnippets));}catch{/* Optional display preferences. */}},[columns,showSnippets]);
  const [locationError, setLocationError] = useState(initial.error);
  const [collections, setCollections] = useState<BibCollection[]>([]);
  const [articles, setArticles] = useState<BibArticle[]>([]);
  const [attachmentSummaries, setAttachmentSummaries] = useState<Record<string, AttachmentSummary>>({});
  const [total, setTotal] = useState(0);
  const [mode, setMode] = useState<SearchMode>(initial.request.mode ?? "lexical");
  const {history,clear:clearHistory}=mode === "hybrid" ? topicHistory : keywordHistory;
  const recordHistoryRef=useRef({lexical:keywordHistory.record,hybrid:topicHistory.record});recordHistoryRef.current={lexical:keywordHistory.record,hybrid:topicHistory.record};
  const [topicDraft, setTopicDraft] = useState(initial.request.semantic_query ?? "");
  const [topic, setTopic] = useState(initial.request.semantic_query ?? "");
  const [retrieval, setRetrieval] = useState<SearchRetrieval | null>(null);
  const [queryDraft, setQueryDraft] = useState(initial.request.expression ? formatExpression(initial.request.expression) : initial.request.query ?? "");
  const [query, setQuery] = useState(initial.request.expression ? formatExpression(initial.request.expression) : initial.request.query ?? "");
  const [expression, setExpression] = useState<SearchExpr | null>(initial.request.expression ?? null);
  const [defaultFields, setDefaultFields] = useState<SearchField[]>(initial.request.default_fields ?? []);
  const [sameChild, setSameChild] = useState(initial.request.scope?.require_same_child ?? false);
  const [libraryId, setLibraryId] = useState<number | null>(initial.request.scope?.library_id ?? null);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [filtersOpen, setFiltersOpen] = useState(hasFilters(initial.request.filters ?? {}));
  const [activeSavedId, setActiveSavedId] = useState<string | null>(initial.savedId);
  const [indexEngine, setIndexEngine] = useState("unknown");
  const [updatesPending, setUpdatesPending] = useState(false);
  const [indexGeneration, setIndexGeneration] = useState("");
  const [filters, setFilters] = useState<SearchFilters>(initial.request.filters ?? {});
  const [sort, setSort] = useState<SearchSort>(initial.request.sort ?? "relevance");
  const [facets, setFacets] = useState<SearchFacets | null>(null);
  const [searchHits, setSearchHits] = useState<Record<string, SearchHit>>({});
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const { searchPage, libraryMetadata, invalidateMetadata } = useBibSearch();
  const listAbort = useRef<AbortController | null>(null);
  const detailAbort = useRef<AbortController | null>(null);
  const detailSequence = useRef(0);
  const composing = useRef(false);
  const [isComposing, setIsComposing] = useState(false);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const searchActive = mode === "hybrid" || Boolean(query.trim()) || Boolean(expression) || hasFilters(filters) || sort !== "relevance" || defaultFields.length > 0 || sameChild || filtersOpen || libraryId !== null || Boolean(locationError);
  const [selectedCollection, setSelectedCollection] = useState<string | null>(initial.request.scope?.unfiled ? "@unfiled" : initial.request.scope?.collection_id ?? null);
  const [includeDescendants, setIncludeDescendants] = useState(() => { if (initial.request.scope?.include_descendants !== undefined) return initial.request.scope.include_descendants; try { return localStorage.getItem("autonomics.collections.recursive") === "true"; } catch { return false; } });
  const [associations, setAssociations] = useState<Array<{ collection_id: string; article_id: string }>>([]);
  const [libraryTotal, setLibraryTotal] = useState(0);
  const [selectedArticle, setSelectedArticle] = useState<string | null>(null);
  const selectionRef=useRef(selectedArticle);selectionRef.current=selectedArticle;
  const [resultContext,setResultContext]=useState("");
  const lastResultContext=useRef("");
  const [attachmentsOpen, setAttachmentsOpen] = useState(true);
  const [detail, setDetail] = useState<BibDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [loading, setLoading] = useState(false);
  const [searchCancelled, setSearchCancelled] = useState(false);
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
  const resultsViewport = useRef<HTMLDivElement | null>(null);
  const buttonColumns=["24px","minmax(0,1fr)",...(wideColumns&&columns.creator?["160px"]:[]),...(columns.year?["56px"]:[]),...(wideColumns&&columns.journal?["160px"]:[]),...(columns.files?[wideColumns?"80px":"56px"]:[])].join(" ");
  const scopeLabel = selectedCollection === "@unfiled" ? "Unfiled" : selectedCollection ? collectionPath(collections, selectedCollection) : "All references";
  const currentRequest = useMemo<SearchRequest>(() => ({
    mode, semantic_query: mode === "hybrid" ? topic : null,
    query: expression ? "" : query, expression, default_fields: defaultFields, filters, sort,
    scope: { library_id: libraryId, collection_id: selectedCollection && selectedCollection !== "@unfiled" ? selectedCollection : null, unfiled: selectedCollection === "@unfiled", include_descendants: includeDescendants, require_same_child: sameChild },
  }), [mode, topic, query, expression, defaultFields, filters, sort, libraryId, selectedCollection, includeDescendants, sameChild]);
  const commitQuery = (text = queryDraft) => { if (text !== query) {setExpression(null);setQuery(text);} else void loadLibrary(query); };
  const commitMainQuery = () => { if (mode === "hybrid") { if (topicDraft !== topic) setTopic(topicDraft.trim()); else void loadLibrary(query); } else commitQuery(); };
  const draftRequest: SearchRequest = { ...currentRequest, ...(queryDraft !== query ? {query:queryDraft, expression:null} : {}), semantic_query: mode === "hybrid" ? topicDraft.trim() : null };
  const clearSearch = () => {setMode("lexical");setTopic("");setTopicDraft("");setLocationError("");setFilters({});setSort("relevance");setDefaultFields([]);setExpression(null);setSameChild(false);setQuery("");setQueryDraft("");setActiveSavedId(null);setFiltersOpen(false);setLibraryId(null);};
  const restoreRequest = (request: SearchRequest, savedId: string | null) => {
    setLocationError("");
    setMode(request.mode ?? "lexical");setTopic(request.semantic_query ?? "");setTopicDraft(request.semantic_query ?? "");
    setQueryDraft(request.expression ? formatExpression(request.expression) : request.query ?? "");
    setQuery(request.expression ? formatExpression(request.expression) : request.query ?? "");
    setExpression(request.expression ?? null);setFilters(request.filters ?? {});setSort(request.sort ?? "relevance");setDefaultFields(request.default_fields ?? []);
    setSelectedCollection(request.scope?.unfiled ? "@unfiled" : request.scope?.collection_id ?? null);
    setIncludeDescendants(request.scope?.include_descendants ?? false);setSameChild(request.scope?.require_same_child ?? false);setLibraryId(request.scope?.library_id ?? null);setActiveSavedId(savedId);setLibraryOpen(false);
  };


  const applySavedSearch = (rule: SavedSearch) => restoreRequest(rule.request, rule.id);
  useEffect(() => {if (!locationError && (mode !== "hybrid" || topic.trim())) writeSearchLocation(currentRequest, activeSavedId);}, [currentRequest, activeSavedId, locationError, mode, topic]);
  useEffect(() => {const restore = () => {const state=readSearchLocation();if(state.error)setLocationError(state.error);else restoreRequest(state.request,state.savedId);};window.addEventListener("popstate",restore);return()=>window.removeEventListener("popstate",restore);}, []);

  const loadLibrary = useCallback(async (
    nextQuery = "",
    options: { append?: boolean; offset?: number; cursor?: string | null } = {},
  ) => {
    if (mode === "hybrid" && !topic.trim()) { requestSequence.current++; listAbort.current?.abort(); setLoading(false); setError(null); return; }
    if (locationError) {setError(locationError);return;}
    const append = options.append ?? false;
    const offset = options.offset ?? 0;
    const requestId = ++requestSequence.current;
    const selectionVersion=detailSequence.current;
    const context=JSON.stringify({...currentRequest,query:expression ? "" : nextQuery});
    listAbort.current?.abort();
    const abort = new AbortController();
    listAbort.current = abort;
    setLoading(true);
    setSearchCancelled(false);
    setError(null);
    try {
      const base = selectedCollection === "@unfiled" ? "/bib/articles/unfiled"
        : selectedCollection ? `/bib/collections/${encodeURIComponent(selectedCollection)}/articles` : "/bib/articles";
      const advanced = Boolean(expression) || hasFilters(filters) || sort !== "relevance" || defaultFields.length > 0 || sameChild || filtersOpen || libraryId !== null;
      const useV2 = mode === "hybrid" || advanced || (indexEngine !== "legacy" && Boolean(nextQuery.trim()));
      const pagePromise = useV2
        ? searchPage({ ...currentRequest, query: expression ? "" : nextQuery, cursor: append ? options.cursor : null }, abort.signal)
        : gatewayRequest("GET", `${base}?query=${encodeURIComponent(nextQuery)}&include_descendants=${includeDescendants}&limit=50&offset=${offset}`, undefined, { signal: abort.signal });
      const [metadataResult, pageResult] = await Promise.allSettled([libraryMetadata(), pagePromise]);
      if (requestId !== requestSequence.current) return;
      if (metadataResult.status === "rejected") throw metadataResult.reason;
      const metadata = metadataResult.value;
      const nextCollections = metadata.collections;
      if (selectedCollection && selectedCollection !== "@unfiled" && !nextCollections.some(c => c.id === selectedCollection && !c.deleted)) {
        setCollections(nextCollections);
        setSelectedCollection(null); setSelectedArticle(null); setDetail(null); setArticles([]);
        return;
      }
      if (pageResult.status === "rejected") throw pageResult.reason;
      const articleResponse = pageResult.value;
      setSearchHits(current => {
        const hits = Object.fromEntries((articleResponse.hits ?? []).map((hit: SearchHit) => [hit.article_id, hit]));
        return append ? { ...current, ...hits } : hits;
      });
      setFacets(articleResponse.facets ?? null);
      setRetrieval(previous => articleResponse.retrieval ? { ...articleResponse.retrieval, hits: append ? { ...previous?.hits, ...articleResponse.retrieval.hits } : articleResponse.retrieval.hits } : null);
      setNextCursor(articleResponse.next_cursor ?? null);
      setIndexGeneration(articleResponse.index_generation ?? "");
      setUpdatesPending(false);
      recordHistoryRef.current[mode](mode === "hybrid" ? topic : nextQuery);
      const nextArticles: BibArticle[] = articleResponse.articles ?? [];
      const nextSummaries = articleResponse.attachment_summaries ?? {};
      if (!append && context!==lastResultContext.current && detailSequence.current===selectionVersion && selectionRef.current && !nextArticles.some(article=>article.id===selectionRef.current)) {
        detailSequence.current++;detailAbort.current?.abort();setSelectedArticle(null);setDetail(null);setDetailLoading(false);
      }
      lastResultContext.current=context;setResultContext(context);
      if (requestId !== requestSequence.current) return;
      setCollections(metadata.collections);
      setLibraryTotal(metadata.total);
      setAssociations(current => append ? [...current, ...(articleResponse.associations ?? [])] : articleResponse.associations ?? []);
      setArticles((current) => append ? [...current, ...nextArticles] : nextArticles);
      setAttachmentSummaries((current) => append
        ? { ...current, ...nextSummaries }
        : nextSummaries);
      setTotal(articleResponse.total ?? (articleResponse.articles ?? []).length);
      setArticleOffset(offset + nextArticles.length);
      setHasMoreArticles(useV2 ? Boolean(articleResponse.next_cursor) : offset + nextArticles.length < (articleResponse.total ?? 0));
    } catch (cause) {
      if (requestId !== requestSequence.current || abort.signal.aborted) return;
      if (append) {setHasMoreArticles(false);setNextCursor(null);setUpdatesPending(true);}
      setError(String(cause));
    } finally {
      if (requestId === requestSequence.current) setLoading(false);
    }
  }, [mode, topic, selectedCollection, includeDescendants, filters, sort, expression, defaultFields, sameChild, filtersOpen, libraryId, indexEngine, currentRequest, locationError, searchPage, libraryMetadata]);

  useEffect(() => {
    if (isComposing) return;
    const timer = window.setTimeout(() => { if (!composing.current && queryDraft !== query) { setExpression(null); setQuery(queryDraft); } }, 250);
    return () => window.clearTimeout(timer);
  }, [queryDraft, query, isComposing]);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!searchInputRef.current?.getClientRects().length) return;
      if (document.querySelector(".zotero-pdf-reader,.zotero-txt-reader,dialog[open]")) return;
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") { event.preventDefault(); searchInputRef.current?.focus(); searchInputRef.current?.select(); }
    };
    window.addEventListener("keydown", onKey);
    return () => { window.removeEventListener("keydown", onKey); listAbort.current?.abort(); detailAbort.current?.abort(); };
  }, []);

  useEffect(() => {
    void loadLibrary(query);
  }, [loadLibrary, query]);

  useEffect(() => {
    const refresh = () => { invalidateMetadata(); void loadLibrary(query); };
    window.addEventListener("focus", refresh);
    return () => window.removeEventListener("focus", refresh);
  }, [loadLibrary, query, invalidateMetadata]);

  useEffect(() => { try { localStorage.setItem("autonomics.collections.recursive", String(includeDescendants)); } catch { /* preference storage may be unavailable */ } }, [includeDescendants]);

  const latestReload=useRef<()=>void>(()=>{});
  latestReload.current=()=>{invalidateMetadata();void loadLibrary(query);};
  useEffect(()=>watchBibCollections(()=>latestReload.current()),[]);

  const loadMoreArticles = useCallback(() => {
    if (loading || !hasMoreArticles) return;
    void loadLibrary(query, { append: true, offset: articleOffset, cursor: nextCursor });
  }, [articleOffset, hasMoreArticles, loadLibrary, loading, query, nextCursor]);

  const latestLoadMore = useRef(loadMoreArticles);
  latestLoadMore.current = loadMoreArticles;
  useEffect(() => {
    const node = articleLoaderRef.current;
    if (!node || !hasMoreArticles || loading) return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) latestLoadMore.current();
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
    const requestId = ++detailSequence.current;
    detailAbort.current?.abort();
    const abort = new AbortController();
    detailAbort.current = abort;
    setSelectedArticle(articleId);
    setAttachmentsOpen(true);
    setDetail(null);
    if (!articleId) { setDetailLoading(false); return; }
    if (!window.matchMedia("(min-width: 1280px)").matches) setDetailsOpen(true);
    setDetailLoading(true);
    void gatewayRequest("GET", `/bib/articles/${encodeURIComponent(articleId)}`, undefined, { signal: abort.signal })
      .then((response) => { if (requestId === detailSequence.current) setDetail(response as BibDetail); })
      .catch((cause) => { if (requestId === detailSequence.current && !abort.signal.aborted) setError(String(cause)); })
      .finally(() => { if (requestId === detailSequence.current) setDetailLoading(false); });
  }, []);

  const selectCollection = (collectionId: string | null) => {
    if (collectionId === selectedCollection) return;
    requestSequence.current++;
    detailSequence.current++;
    detailAbort.current?.abort();
    setDetailLoading(false);
    setSelectedCollection(collectionId);
    setArticles([]);
    setSelectedArticle(null);
    setDetail(null);
    setLibraryOpen(false);
    setActiveSavedId(null);
  };

  const collectionAction = async (action: CollectionAction) => {
    const response = await gatewayRequest(action.method, action.path, action.body);
    invalidateMetadata();
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
      setExpression(null);
      invalidateMetadata();
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
      const response = searchActive && !checkedIds.length ? await gatewayRequest("POST", "/bib/search/export", {format:"bibtex", search:currentRequest}) : await gatewayRequest("GET", `/bib/export?format=bibtex&query=${encodeURIComponent(query)}${selectedCollection === "@unfiled" ? "&unfiled=true" : ""}${checkedIds.length ? `&ids=${encodeURIComponent(checkedIds.join(","))}` : ""}${!checkedIds.length && selectedCollection && selectedCollection !== "@unfiled" ? `&collection_id=${encodeURIComponent(selectedCollection)}&include_descendants=${includeDescendants}` : ""}`);
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
  const openReaderFor = (article: BibArticle, format?: string | null, attachmentKey?: string | null, search?: import("@/reader-target").ReaderSearchTarget, expectedHash?: string | null) => {
    readerRef.current?.close();
    readerRef.current = null;
    const openReader = format === "txt" ? openTxtReader : openPdfReader;
    void openReader({ articleId: article.id, title: article.title, article, attachmentKey, search, expectedHash })
      .then((handle) => { readerRef.current = handle; })
      .catch((cause) => {if (!(cause instanceof DOMException && cause.name === "AbortError")) setError(String(cause));});
  };
  const openEvidence = async (evidence: SearchEvidence) => {
    if (!selected) return;
    try {
      await gatewayRequest("POST", `/bib/articles/${encodeURIComponent(selected.id)}/search-evidence`, {search:{...currentRequest,cursor:null,page_size:1},index_generation:evidence.content_revision});
      if (!evidence.attachment_key) {
        const note = evidence.annotation_id ? document.getElementById(`bib-note-${evidence.annotation_id}`) : null;
        note?.scrollIntoView({block:"nearest"});note?.focus();return;
      }
      const first=evidence.highlight_ranges[0];
      const query=first ? Array.from(evidence.snippet).slice(first.start,first.end).join("") : evidence.matched_query;
      openReaderFor(selected,null,evidence.attachment_key,{query,page:evidence.page,annotationId:evidence.annotation_id},evidence.attachment_hash);
    } catch (failure) {setError(String(failure));}
  };
  const readFulltext = (format: string | null | undefined, attachmentKey?: string | null) => {
    if (!selected) return;
    openReaderFor(selected, format, attachmentKey);
  };
  const withReader = (entry: AttachmentEntry): AttachmentEntry =>
    entry.kind === "pdf" || entry.kind === "text"
      ? { ...entry, onClick: () => readFulltext(entry.format, entry.attachmentKey) }
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
      invalidateMetadata();
      await loadLibrary(query);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="grid min-h-0 flex-1 grid-rows-[minmax(0,1fr)] gap-2 xl:grid-cols-[216px_minmax(0,1fr)_336px] xl:grid-rows-1">
      {advancedOpen && <AdvancedSearchBuilder query={queryDraft} expression={queryDraft === query ? expression : null} onClose={() => setAdvancedOpen(false)} onApply={(expr,text) => {setExpression(expr);setQuery(text);setQueryDraft(text);setAdvancedOpen(false);}} />}
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
          "max-xl:fixed max-xl:top-13 max-xl:bottom-0 max-xl:left-0 max-xl:z-[35] max-xl:w-[min(88vw,300px)] max-xl:rounded-none max-xl:shadow-2xl max-xl:transition-transform",
          libraryOpen ? "max-xl:translate-x-0" : "max-xl:-translate-x-full",
        )}
      >
        <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b px-3">
          <span className="text-xs font-semibold tracking-wide uppercase">Library</span>
          <div className="flex shrink-0 items-center gap-1">
            <Badge variant="secondary" aria-label="Articles in library" title="All articles, independent of the current search">{libraryTotal.toLocaleString()}</Badge>
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
          <SavedSearches request={draftRequest} scopeLabel={scopeLabel} activeId={activeSavedId} disabled={indexEngine === "legacy" || (mode === "hybrid" && !topicDraft.trim())} onSelect={applySavedSearch} onSaved={applySavedSearch} onDeleted={id => {if (id === activeSavedId) setActiveSavedId(null);}} />
          <SearchIndexStatus onEngine={setIndexEngine} onSourceChange={() => {if (!loading && articleOffset <= 50) latestReload.current();else setUpdatesPending(true);}} />
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
              if (!composing.current) commitMainQuery();

            }}
          >
            <Search className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              ref={searchInputRef}
              aria-label="Search bibliography"
              value={mode === "hybrid" ? topicDraft : queryDraft}
              onCompositionStart={() => { composing.current = true; setIsComposing(true); }}
              onCompositionEnd={(event) => { composing.current = false; setIsComposing(false); if (mode === "hybrid") setTopicDraft(event.currentTarget.value); else setQueryDraft(event.currentTarget.value); }}
              placeholder={mode === "hybrid" ? "Describe a research topic, then press Enter" : `Search ${total.toLocaleString()} items`}
              className="h-7 pl-8 text-[13px]"
              onChange={(event) => {setLocationError("");if (mode === "hybrid") setTopicDraft(event.target.value); else setQueryDraft(event.target.value);}}
              onKeyDown={(event) => {
                if (event.key === "ArrowDown" && !isComposing && !loading && articles.length) {event.preventDefault();resultsViewport.current?.scrollTo({top:0});selectArticle(articles[0].id);requestAnimationFrame(()=>resultsViewport.current?.querySelector<HTMLButtonElement>("[data-bib-row-primary]")?.focus());}
                if (event.key === "Escape") { if (mode === "hybrid") {setTopicDraft("");setTopic("");} else {setQueryDraft("");setQuery("");setExpression(null);} }
                if (event.key === "Enter" && !event.nativeEvent.isComposing && !composing.current) {
                  event.preventDefault();
                  commitMainQuery();

                }
              }}
            />
          </form>
          {mode === "hybrid" && <Button size="xs" variant="secondary" disabled={!topicDraft.trim() || isComposing} onClick={commitMainQuery}>Search topic</Button>}
          {mode === "hybrid" && loading && <Button size="xs" variant="ghost" aria-label="Cancel search" onClick={() => {requestSequence.current++;listAbort.current?.abort();setLoading(false);setError(null);setSearchCancelled(true);}}>Cancel</Button>}
          <Button size="xs" variant="ghost" disabled={indexEngine === "legacy"} onClick={() => setAdvancedOpen(true)}>Advanced</Button>
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
          <Button size="icon-sm" variant="ghost" aria-label="Export BibTeX" title={retrieval ? "Export retrieved candidates as BibTeX" : "Export BibTeX"} onClick={() => void exportLibrary()} disabled={loading || busy === "export" || (mode === "hybrid" && resultContext !== JSON.stringify(currentRequest))}>
            <Download />
          </Button>
        </div>

        <div className="flex flex-wrap items-center gap-2 border-b px-2 py-1.5 text-xs">
          <select aria-label="Search mode" className="rounded border bg-background p-0.5" disabled={indexEngine === "legacy"} value={mode} onChange={event => {setLocationError("");setMode(event.target.value as SearchMode);}}><option value="lexical">Keywords</option><option value="hybrid">Topic · experimental</option></select>
          <span className="min-w-0 truncate text-muted-foreground">{scopeLabel}{selectedCollection && includeDescendants ? " · including subcollections" : ""}</span>
          <select aria-label="Quick search fields" title="Default fields for words without an explicit field" className="max-w-44 rounded border bg-background p-0.5" disabled={indexEngine === "legacy"} value={Object.keys(quickFields).find(key => JSON.stringify(quickFields[key]) === JSON.stringify(defaultFields)) ?? "custom"} onChange={event => setDefaultFields(quickFields[event.target.value] ?? [])}><option value="all">Everything</option><option value="title">Title, year, author</option><option value="metadata">Metadata and notes</option><option value="custom" disabled>Custom fields</option></select>
          <Button size="xs" variant={filtersOpen ? "secondary" : "ghost"} disabled={indexEngine === "legacy"} aria-expanded={filtersOpen} onClick={() => setFiltersOpen(!filtersOpen)}>Filters</Button>
          <label className="flex items-center gap-1" title="Positive conditions must match the same evidence object"><input type="checkbox" disabled={indexEngine === "legacy"} checked={sameChild} onChange={event => setSameChild(event.target.checked)} />Same attachment</label>
          <SearchAssist topic={mode === "hybrid"} history={history} onClear={clearHistory} onPick={text=>{if (mode === "hybrid") {setTopicDraft(text);setTopic(text);} else {setQueryDraft(text);commitQuery(text);}searchInputRef.current?.focus();}} />
          <ColumnsPicker columns={columns} onChange={setColumns} snippets={showSnippets} onSnippets={setShowSnippets} />
          <SaveSnapshotButton execution={{ kind: "library", search: currentRequest }} disabled={indexEngine === "legacy" || loading || (mode === "hybrid" && !topic.trim())} />
          <span role="status" className="ml-auto text-muted-foreground">{loading ? "Searching…" : `${total.toLocaleString()} ${retrieval ? "candidate articles" : `article${total === 1 ? "" : "s"}`}`}</span>
          <select aria-label="Search sort" disabled={indexEngine === "legacy"} className="rounded border bg-background px-1 py-0.5" value={sort} onChange={event => setSort(event.target.value as SearchSort)}><option value="relevance">Relevance</option><option value="newest">Newest</option><option value="oldest">Oldest</option><option value="title">Title</option></select>
          {searchActive && <Button size="xs" variant="ghost" onClick={clearSearch}>Clear search</Button>}
        </div>
        {mode === "hybrid" && <>
          <div className="space-y-1 border-b px-2 py-2 text-xs">
            <label className="flex flex-wrap items-center gap-2">Required conditions<Input aria-label="Required search conditions" className="h-7 min-w-40 flex-1 text-xs" value={queryDraft} placeholder="Optional: year:2020..2026 NOT tag:excluded" onChange={event => {setLocationError("");setQueryDraft(event.target.value);}} onCompositionStart={() => {composing.current=true;setIsComposing(true);}} onCompositionEnd={event => {composing.current=false;setIsComposing(false);setQueryDraft(event.currentTarget.value);}} onKeyDown={event => {if (event.key === "Enter" && !event.nativeEvent.isComposing && !composing.current) {event.preventDefault();commitQuery();}}} /></label>
            <p className="text-muted-foreground">Required conditions, scope and filters always apply. The topic is a natural-language description, not Boolean syntax.</p>
            {(!topic.trim() || topicDraft.trim() !== topic) && <p role="status">Enter a topic and submit it to apply your changes.</p>}
          </div>
          <SemanticSearchControls onInvalidate={() => {setUpdatesPending(true);setNextCursor(null);setHasMoreArticles(false);}} onReady={() => {if (!loading && articleOffset <= 50) latestReload.current();else setUpdatesPending(true);}} onKeyword={() => setMode("lexical")} />
        </>}
        {resultContext && resultContext !== JSON.stringify(currentRequest) && <p role="status" className="border-b px-2 py-1 text-xs text-muted-foreground">Previous results are retained until the new search succeeds.</p>}
        {mode === "hybrid" && searchCancelled && <p role="status" className="border-b px-2 py-1 text-xs text-muted-foreground">Search cancelled. Previous results are retained.</p>}
        {retrieval && <p className="border-b px-2 py-1 text-xs text-muted-foreground">Retrieved candidate pool · {retrieval.eligible_articles.toLocaleString()} articles satisfy the required conditions. Facet counts describe the required query and filters; sorting and export apply to retrieved candidates.</p>}
        {filtersOpen && <SearchFiltersPanel value={filters} facets={facets} onChange={setFilters} />}
        {updatesPending && <div className="flex items-center justify-between border-b px-2 py-1 text-xs">Library or search index updated<Button size="xs" variant="ghost" onClick={() => void loadLibrary(query)}>Refresh results</Button></div>}
        {facets && !filtersOpen && <div className="flex flex-wrap items-center gap-2 border-b px-2 py-1.5 text-xs">
          <select aria-label="Exact tag filter" value={filters.tag_ids?.[0] ?? ""} onChange={event => setFilters(current => ({...current,tag_ids:event.target.value ? [Number(event.target.value)] : []}))} className="max-w-40 rounded border bg-background p-1"><option value="">All tags</option>{facets.tags.map(value => <option key={value.value} value={value.value}>{value.label} ({value.count})</option>)}</select>
          <select aria-label="Exact creator filter" value={filters.creator_ids?.[0] ?? ""} onChange={event => setFilters(current => ({...current,creator_ids:event.target.value ? [Number(event.target.value)] : []}))} className="max-w-40 rounded border bg-background p-1"><option value="">All authors</option>{facets.creators.map(value => <option key={value.value} value={value.value}>{value.label} ({value.count})</option>)}</select>
          <label className="flex items-center gap-1"><input type="checkbox" checked={filters.has_fulltext === true} onChange={event => setFilters(current => ({...current,has_fulltext:event.target.checked ? true : null}))}/>Has full text</label>
          <label className="flex items-center gap-1"><input type="checkbox" checked={filters.has_annotations === true} onChange={event => setFilters(current => ({...current,has_annotations:event.target.checked ? true : null}))}/>Has notes</label>
        </div>}
        {error && <div role="alert" className="border-b border-destructive/30 bg-destructive/10 p-2 text-xs text-destructive">{error}<Button size="xs" variant="ghost" onClick={() => void loadLibrary(query)}>Retry</Button></div>}
        <div style={{gridTemplateColumns:`24px ${buttonColumns}`}} className="grid h-8 shrink-0 items-center border-b bg-muted/60 px-2 text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
          <Checkbox
            aria-label="Select all visible articles"
            checked={allVisibleChecked}
            indeterminate={!allVisibleChecked && someVisibleChecked}
            onCheckedChange={(checked) => toggleAllChecked(checked === true)}
          />
          <span />
          <span>Title{checkedIds.length ? ` (${checkedIds.length} selected)` : ""}</span>
          {wideColumns && columns.creator && <span>Creator</span>}
          {columns.year && <span>Year</span>}
          {wideColumns && columns.journal && <span>Journal</span>}
          {columns.files && <span className="text-center">Files</span>}
        </div>

        <ScrollArea className="min-h-0 flex-1" contentClassName="pb-6" viewportRef={resultsViewport} viewportLabel="Literature results viewport" viewportTabIndex={0} viewportClassName="focus-visible:ring-1 focus-visible:ring-ring" tabIndex={-1}>
          {loading && !visibleArticles.length && (
            <div className="space-y-px">
              {Array.from({ length: 10 }).map((_, index) => (
                <div key={index} className="h-9 animate-pulse border-b bg-muted/30" />
              ))}
            </div>
          )}

          <VirtualArticleList articles={visibleArticles} total={total} viewport={resultsViewport} contextKey={JSON.stringify({request:currentRequest,snippets:showSnippets})} estimatedHeight={showSnippets && Boolean(query.trim() || expression) ? 76 : 37} selectedId={selectedArticle} onNavigate={selectArticle} onToggle={id => toggleChecked(id, !checkedIds.includes(id))}>{(article) => {
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
                    style={{gridTemplateColumns:"24px minmax(0,1fr)"}}
                    className={cn(
                      "grid items-center",
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
                      data-bib-row-primary
                      tabIndex={isSelected || (!selectedArticle && visibleArticles[0]?.id === article.id) ? 0 : -1}
                      draggable
                      onDragStart={event => { event.dataTransfer.setData("application/x-autonomics-article", article.id); event.dataTransfer.effectAllowed = "copy"; }}
                      aria-expanded={isSelected && attachmentsOpen}
                      onClick={() => isSelected ? setAttachmentsOpen(!attachmentsOpen) : selectArticle(article.id)}
                      style={{gridColumn:"2",gridTemplateColumns:buttonColumns}}
                      className="grid h-9 w-full items-center px-2 text-left transition-colors hover:bg-accent/70"
                    >
                      <span className="flex items-center justify-center">
                        {isSelected && attachmentsOpen ? (
                          <ChevronDown className="size-3.5 text-muted-foreground" />
                        ) : (
                          <ChevronRight className="size-3.5 text-muted-foreground/60" />
                        )}
                      </span>
                      <span className="min-w-0 truncate pr-3 text-[13px]">{article.title}</span>
                      {wideColumns && columns.creator && <span className="min-w-0 truncate pr-3 text-[13px] text-muted-foreground">{creatorDisplay(article)}</span>}
                      {columns.year && <span className="text-xs text-muted-foreground">{article.year ?? "—"}</span>}
                      {wideColumns && columns.journal && <span className="min-w-0 truncate pr-3 text-xs text-muted-foreground">{article.journal ?? "—"}</span>}
                      {columns.files && <span className="flex items-center justify-center text-muted-foreground"><AttachmentTypeIcons summary={summary} /></span>}
                    </button>
                  </div>

                  {showSnippets && Boolean(query.trim() || expression || retrieval) && searchHits[article.id] && <div className="px-9 pb-2 text-xs text-muted-foreground">
                    <RecallLabel recall={retrieval?.hits[article.id]} />
                    <span className="mr-2 font-medium">{searchHits[article.id].matched_field}</span>
                    <SearchSnippet text={searchHits[article.id].snippet} ranges={searchHits[article.id].highlight_ranges} />
                    <Button size="xs" variant="ghost" onClick={() => selectArticle(article.id)}>View evidence</Button>
                  </div>}
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
          }}</VirtualArticleList>

          {!loading && !visibleArticles.length && (
            <div className="grid h-full place-items-center text-sm text-muted-foreground">{error ? "Search could not run. Correct the query or retry." : "No matching articles"}</div>
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
          "max-xl:fixed max-xl:top-13 max-xl:bottom-0 max-xl:right-0 max-xl:z-[35] max-xl:w-[min(94vw,360px)] max-xl:rounded-none max-xl:shadow-2xl max-xl:transition-transform",
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
              {searchActive && indexGeneration && resultContext===JSON.stringify(currentRequest) && articles.some(article=>article.id===selected.id) && <SearchEvidencePanel articleId={selected.id} request={currentRequest} generation={indexGeneration} onOpen={evidence => void openEvidence(evidence)} />}
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

              <LiteratureExploration article={selected} importTarget={selectedCollection && selectedCollection !== "@unfiled" ? { collection_id: selectedCollection } : { library_id: libraryId ?? 1 }} targetLabel={scopeLabel} onImported={() => { invalidateMetadata(); void loadLibrary(query); }} />

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
              <DiscoveryProvenance key={selected.id} articleId={selected.id} />

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
                      <article id={`bib-note-${note.id}`} tabIndex={-1} key={note.id} className="rounded-md border p-2">
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
              onClick={() => readFulltext(activeDetail?.fulltext?.file_format, activeDetail?.fulltext?.attachment_key)}
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
