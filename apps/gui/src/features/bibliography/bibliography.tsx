import { ManuscriptsWorkspace } from "./manuscripts";
import { AddToContext, useContextBasket } from "@/features/context/context-basket";
import { articleContext } from "@/features/context/context-model";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import {
  BookOpen,
  ChevronDown,
  ChevronRight,
  Download,
  Copy,
  Quote,
  ExternalLink,
  Camera,
  File,
  FileQuestion,
  FileText,
  FileX2,
  Link2,
  LoaderCircle,
  NotebookText,
  Plus,
  PanelLeft,
  PanelRight,
  Pencil,
  Search,
  StickyNote,
  Star,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { downloadBibAttachment, gatewayRequest, pickUploadFiles, saveTextFile, uploadBibAttachment, watchBibCollections } from "@/gateway";
import { parseImportIdentifier } from "./import-identifier";
import { ZoteroBundleImport } from "./zotero-bundle-import";
import { BrowserConnectorHelp } from "./browser-connector-help";
import { LibraryTransferDialog } from "./library-transfer-dialog";
import { DuplicateReview } from "./duplicate-review";
import { BatchOrganize } from "./batch-organize";
import { ArticleTrash } from "./article-trash";
import { MetadataEditor, extraFieldLabels } from "./metadata-editor";
import { CollectionTree, type CollectionAction } from "./collection-tree";
import { collectionPath } from "./collection-tree-model";
import { openPdfReader, openTxtReader, hasReaderOpener, openReaderEditor } from "@/features/reader";
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
import {
  DiscoveryProvenance,
  DiscoveryView,
  LiteratureExploration,
  RssSubscriptions,
} from "@/features/discovery";
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
  filename?: string;
  expectedHash?: string | null;
  onDownload?: () => void;
  downloading?: boolean;
  onRemove?: () => void;
  onPrefer?: () => void;
  preferred?: boolean;
  mutating?: boolean;
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

function creatorDisplay(article: BibArticle, unknownLabel: string, etAl: (surname: string) => string) {
  const authors = article.authors ?? [];
  if (!authors.length) return article.source || article.journal || unknownLabel;
  if (authors.length === 1) return authorName(authors[0]);
  const first = authors[0];
  const surname = first.last_name || first.name?.split(/\s+/).slice(-1)[0] || first.name;
  return surname ? etAl(surname) : unknownLabel;
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

function attachmentFileName(path: string | null | undefined, fullTextLabel: string) {
  if (!path) return fullTextLabel;
  const normalized = path.split(/[\\/]/).filter(Boolean).pop();
  if (!normalized) return fullTextLabel;
  try { return decodeURIComponent(normalized); } catch { return normalized; }
}

function attachmentEntries(article: BibArticle, detail: BibDetail | null, t: TFunction): AttachmentEntry[] {
  const entries: AttachmentEntry[] = [];
  const fullText = detail?.fulltext;

  if (detail?.attachments?.length) {
    for (const attachment of detail.attachments) {
      const format = /pdf/i.test(attachment.content_type ?? "") || /\.pdf$/i.test(attachment.original_filename ?? "") ? "pdf" : "txt";
      if (attachment.link_mode === 3 && attachment.source_url) entries.push({key:attachment.key,kind:"link",label:attachment.title || t("main.linkedFile"),caption:attachment.source_url,href:attachment.source_url});
      else entries.push({key:attachment.key,attachmentKey:attachment.key,preferred:attachment.key === fullText?.attachment_key,filename:attachment.original_filename || attachmentFileName(attachment.path || attachment.vfs_path, t("main.fullText")),expectedHash:attachment.storage_hash,kind:format === "pdf" ? "pdf" : "text",format,label:attachment.title || attachment.original_filename || t("main.attachment"),caption:[format.toUpperCase(),formatFileSize(attachment.file_size)].filter(Boolean).join(" · ")});
    }
  } else if (fullText) {
    const format = fullText.file_format?.toLowerCase() ?? null;
    entries.push({
      key: "fulltext",
      attachmentKey: fullText.attachment_key,
      filename: attachmentFileName(fullText.file_path, t("main.fullText")),
      kind: format === "txt" ? "text" : "pdf",
      format,
      label: attachmentFileName(fullText.file_path, t("main.fullText")),
      caption: [
        fullText.file_format?.toUpperCase() || t("main.file"),
        formatFileSize(fullText.file_size),
        fullText.extract_status === "done" ? t("main.indexed") : fullText.extract_status || null,
      ].filter(Boolean).join(" · "),
    });
  }

  const identifierLabels: Record<string, string> = {
    pmid: t("main.pubmedEntry"),
    doi: t("main.doiEntry"),
    pmc: t("main.pmcEntry"),
    arxiv: t("main.arxivEntry"),
    openalex: t("main.openalexEntry"),
  };
  const identifierKinds = ["pmid", "doi", "pmc", "arxiv", "openalex"];
  for (const kind of identifierKinds) {
    const value = articleIdentifier(article, kind);
    const href = value ? identifierHref(kind, value) : null;
    if (!value || !href) continue;
    entries.push({
      key: `identifier-${kind}`,
      kind: "link",
      label: identifierLabels[kind] ?? t("main.identifierEntry", { kind }),
      caption: value,
      href,
    });
  }

  if (detail?.annotations.length) {
    entries.push({
      key: "notes",
      kind: "notes",
      label: t("main.notesLabel"),
      caption: t("main.itemCount", { count: detail.annotations.length }),
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
  const { t } = useTranslation("bibliography");
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
            aria-label={t("main.openLabelAria", { label })}
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
  action,
  children,
}: {
  title: string;
  count?: number | null;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="border-b border-border/70 pb-3 last:border-b-0">
      <div className="flex h-8 items-center gap-1.5 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">
        {title}
        {count != null && <span className="text-[11px] opacity-70">{count}</span>}
        {action && <span className="ml-auto flex items-center">{action}</span>}
      </div>
      {children}
    </section>
  );
}


function AttachmentRow({ entry }: { entry: AttachmentEntry }) {
  const { t } = useTranslation("bibliography");
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
    <div className="flex min-w-0 items-center gap-1">
      <button type="button" onClick={entry.onClick} className={cn(className, "min-w-0 flex-1")} disabled={!entry.onClick}>
        {content}
      </button>
      {entry.onPrefer && <Button type="button" size="icon-xs" variant="ghost" className="shrink-0"
        aria-label={t("main.setPreferredAria", { label: entry.label })} title={entry.preferred ? t("main.preferredAttachment") : t("main.setPreferredAttachment")}
        disabled={entry.mutating || entry.preferred} onClick={entry.onPrefer}><Star className={entry.preferred ? "fill-current" : ""} /></Button>}
      {entry.onDownload && (
        <Button type="button" size="icon-sm" variant="ghost" className="shrink-0"
          aria-label={t("main.downloadAria", { label: entry.label })} title={entry.downloading ? t("main.downloading") : t("main.downloadTitle", { name: entry.filename || entry.label })}
          disabled={entry.downloading} aria-busy={entry.downloading}
          onClick={entry.onDownload}>
          {entry.downloading ? <LoaderCircle className="animate-spin" /> : <Download />}
        </Button>
      )}
      {entry.onRemove && <Button type="button" size="icon-xs" variant="ghost" className="shrink-0"
        aria-label={t("main.deleteAttachmentAria", { label: entry.label })} title={t("main.deleteAttachmentTitle")} disabled={entry.mutating} onClick={entry.onRemove}><Trash2 /></Button>}
    </div>
  );
}

function AttachmentTypeIcons({ summary }: { summary: AttachmentSummary }) {
  const { t } = useTranslation("bibliography");
  const items = [
    { key: "pdf", show: summary.pdf, icon: FileText, label: t("main.pdfAttachment") },
    { key: "snapshot", show: summary.snapshot, icon: Camera, label: t("main.htmlSnapshot") },
    { key: "link", show: summary.link, icon: Link2, label: t("main.linkedAttachment") },
    { key: "file", show: summary.file, icon: File, label: t("main.storedFile") },
    { key: "note", show: summary.note, icon: StickyNote, label: t("main.notesLabel") },
  ].filter((item) => item.show);

  if (!items.length) {
    return (
      <FileQuestion
        aria-label={t("main.noAttachments")}
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

export type BibliographyWorkspace = "library" | "discover" | "subscriptions" | "manuscripts";

// The workspace tabs live in the app sidebar; this component is controlled by
// the selected workspace and only keeps the lazy-mount bookkeeping: a panel
// is created on first visit, then stays mounted (hidden) so its state
// survives navigation.
export function BibliographyView({ workspace = "library", active = true, focusArticleId, focusNonce, sidebarSlot, externalSidebar = false, onOpenSidebar }: { workspace?: BibliographyWorkspace; active?: boolean; focusArticleId?: string | null; focusNonce?: number | null; sidebarSlot?: HTMLElement | null; externalSidebar?: boolean; onOpenSidebar?: () => void } = {}) {
  const [mounted, setMounted] = useState<Set<BibliographyWorkspace>>(() => new Set([workspace]));
  useEffect(() => {
    setMounted(current => (current.has(workspace) ? current : new Set([...current, workspace])));
  }, [workspace]);
  return <div className="flex min-h-0 flex-1 flex-col">
    {mounted.has("manuscripts") && <div hidden={workspace !== "manuscripts"} className={workspace === "manuscripts" ? "flex min-h-0 flex-1 flex-col" : "hidden"}><ManuscriptsWorkspace active={active && workspace === "manuscripts"} sidebarSlot={workspace === "manuscripts" && active ? sidebarSlot : null} externalSidebar={externalSidebar} onOpenSidebar={onOpenSidebar} /></div>}
    {mounted.has("library") && <div hidden={workspace !== "library"} className={workspace === "library" ? "flex min-h-0 flex-1 flex-col" : "hidden"}><LibraryView focusArticleId={workspace === "library" ? focusArticleId ?? null : null} focusNonce={focusNonce} sidebarSlot={workspace === "library" ? sidebarSlot : null} externalSidebar={externalSidebar} onOpenSidebar={onOpenSidebar} /></div>}
    {mounted.has("discover") && <div hidden={workspace !== "discover"} className={workspace === "discover" ? "flex min-h-0 flex-1 flex-col" : "hidden"}><DiscoveryView /></div>}
    {mounted.has("subscriptions") && <div hidden={workspace !== "subscriptions"} className={workspace === "subscriptions" ? "flex min-h-0 flex-1 flex-col" : "hidden"}><RssSubscriptions active={active && workspace === "subscriptions"} sidebarSlot={workspace === "subscriptions" ? sidebarSlot : null} externalSidebar={externalSidebar} onOpenSidebar={onOpenSidebar} /></div>}
  </div>;
}

function LibraryView({ focusArticleId, focusNonce, sidebarSlot, externalSidebar, onOpenSidebar }: { focusArticleId?: string | null; focusNonce?: number | null; sidebarSlot?: HTMLElement | null; externalSidebar: boolean; onOpenSidebar?: () => void }) {
  const { t } = useTranslation("bibliography");
  const contextBasket = useContextBasket();
  const contextBasketRef = useRef(contextBasket);
  contextBasketRef.current = contextBasket;
  const [initial] = useState(readSearchLocation);
  const keywordHistory=useQueryHistory();
  const topicHistory=useQueryHistory("autonomics.bib.topicHistory.v1");
  const [columns,setColumns]=useState(readResultColumns);
  const [showSnippets,setShowSnippets]=useState(()=>{try{return localStorage.getItem("autonomics.bib.snippets")!=="false";}catch{return true;}});
  const libraryContainer = useRef<HTMLDivElement>(null);
  const [containerWidth, setContainerWidth] = useState(0);
  useEffect(() => {
    const element = libraryContainer.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry.contentRect.width > 0) setContainerWidth(entry.contentRect.width);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const wideColumns = containerWidth >= 760;
  const roomy = containerWidth >= 1180;

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
  /** Deep-linked article id (agent bib:// link) that must survive list reloads
   *  until the loaded list actually contains it. */
  const focusGuard = useRef<string | null>(null);
  const lastFocusArticle = useRef<string | null>(null);
  const [revealId, setRevealId] = useState<string | null>(null);
  const [resultContext,setResultContext]=useState("");
  const lastResultContext=useRef("");
  const [downloadingAttachments, setDownloadingAttachments] = useState<Set<string>>(new Set());
  /** A file drag is hovering the entry-details pane (drop = attach to record). */
  const [detailDropActive, setDetailDropActive] = useState(false);
  const downloadLocks = useRef(new Set<string>());
  const [attachmentsOpen, setAttachmentsOpen] = useState(true);
  const [editingArticle, setEditingArticle] = useState<BibArticle | null>(null);
  const [editingRevision, setEditingRevision] = useState<string | undefined>();
  const [creatingArticle, setCreatingArticle] = useState(false);
  const [transferMode, setTransferMode] = useState<"import" | "export" | null>(null);
  const [exportIds, setExportIds] = useState<string[]>([]);
  const [duplicatesOpen, setDuplicatesOpen] = useState(false);
  const [trashOpen, setTrashOpen] = useState(false);
  const [organizingIds, setOrganizingIds] = useState<string[] | null>(null);
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
  const scopeLabel = selectedCollection === "@unfiled" ? t("main.unfiledScope") : selectedCollection ? collectionPath(collections, selectedCollection) : t("main.allReferences");
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
      if (focusGuard.current && nextArticles.some(article=>article.id===focusGuard.current)) focusGuard.current = null;
      if (!append && context!==lastResultContext.current && detailSequence.current===selectionVersion && selectionRef.current && selectionRef.current!==focusGuard.current && !nextArticles.some(article=>article.id===selectionRef.current)) {
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
  // A new search context invalidates the pending reveal: the revealed row
  // may be filtered out, and re-scrolling to it would fight the user.
  useEffect(() => { setRevealId(null); }, [JSON.stringify({request:currentRequest,snippets:showSnippets})]);
  // When a deep-linked record reaches the loaded list, reveal it: scroll the
  // row into view and hand it the keyboard focus.
  useEffect(() => {
    const target = focusGuard.current;
    if (target && visibleIds.includes(target)) {
      focusGuard.current = null;
      setRevealId(target);
    }
  }, [visibleIds]);
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
    selectionRef.current = articleId;
    setSelectedArticle(articleId);
    setAttachmentsOpen(true);
    setDetail(null);
    if (!articleId) { setDetailLoading(false); return; }
    setDetailsOpen(true);
    setDetailLoading(true);
    void gatewayRequest("GET", `/bib/articles/${encodeURIComponent(articleId)}`, undefined, { signal: abort.signal })
      .then((response) => { if (requestId === detailSequence.current) setDetail(response as BibDetail); })
      .catch((cause) => { if (requestId === detailSequence.current && !abort.signal.aborted) setError(String(cause)); })
      .finally(() => { if (requestId === detailSequence.current) setDetailLoading(false); });
  }, []);

  // Deep link (bare bib:// agent link). In-view targets are selected and
  // revealed directly; out-of-view targets switch to an exact identifier
  // query instead of paging through the whole library. The nonce makes
  // repeated clicks on the same link re-run the focus.
  useEffect(() => {
    const key = `${focusNonce ?? 0}:${focusArticleId ?? ""}`;
    if (!focusArticleId || lastFocusArticle.current === key) return;
    lastFocusArticle.current = key;
    setRevealId(null);
    if (visibleIds.includes(focusArticleId)) {
      focusGuard.current = null;
      selectArticle(focusArticleId);
      setRevealId(focusArticleId);
      return;
    }
    focusGuard.current = focusArticleId;
    setMode("lexical");
    selectCollection(null);
    setExpression({ op: "term", field: "identifier", value: focusArticleId });
    setQuery("");
    setQueryDraft(`identifier:${focusArticleId}`);
    selectArticle(focusArticleId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusArticleId, focusNonce]);

  const selectCollection = (collectionId: string | null) => {
    if (externalSidebar) onOpenSidebar?.();
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
  const attachments = selected ? attachmentEntries(selected, activeDetail, t) : [];
  const identifierRows = [
    { kind: "doi", label: "DOI" },
    { kind: "pmid", label: "PMID" },
    { kind: "pmc", label: "PMCID" },
    { kind: "arxiv", label: "arXiv" },
    { kind: "openalex", label: "OpenAlex" },
  ];

  const importArticle = async () => {
    const identifier = window.prompt(t("main.importPrompt"));
    if (!identifier) return;
    setBusy("import");
    setError(null);
    try {
      const response = await gatewayRequest("POST", "/bib/articles/import", {
        ...parseImportIdentifier(identifier),
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



  const readerRef = useRef<{ close: () => void } | null>(null);
  const libraryAlive = useRef(true);
  useEffect(() => {
    libraryAlive.current = true;
    return () => { libraryAlive.current = false; readerRef.current?.close(); };
  }, []);
  const openReaderFor = (article: BibArticle, format?: string | null, attachmentKey?: string | null, search?: import("@/features/reader/reader-target").ReaderSearchTarget, expectedHash?: string | null) => {
    // Workbench mode: the reader opens as its own editor tab (one session
    // per reader, reopened from the persisted position on reactivation).
    if (hasReaderOpener()) {
      openReaderEditor({ articleId: article.id, title: article.title, attachmentKey, expectedHash, search });
      return;
    }
    // Mount-harness fallback (feature tests without the workbench shell):
    // host the reader inside the library container as before.
    readerRef.current?.close();
    readerRef.current = null;
    const openReader = format === "txt" ? openTxtReader : openPdfReader;
    void openReader({ articleId: article.id, title: article.title, article, attachmentKey, search, expectedHash, supersede: true,
      host: contextBasket ? libraryContainer.current ?? undefined : undefined,
      onReference: contextBasket ? selection => {
        const basket = contextBasketRef.current;
        if (!basket?.available) { window.alert(t("main.selectAgentSessionFirst")); return; }
        basket.add({ kind: "evidence", id: JSON.stringify([article.id, attachmentKey, selection.page, selection.start, selection.end, selection.text]), title: `${article.title} · ${t("main.selectedText")}`, scope: t("main.readerSelectionScope"), source: `bib:${article.id}${attachmentKey ? ` · ${attachmentKey}` : ""}${selection.page ? t("main.pageN", { page: selection.page }) : ""}${selection.start !== undefined ? t("main.charRange", { start: selection.start, end: selection.end }) : ""}`, content: selection.text });
      } : undefined,
    })
      .then((handle) => { if (libraryAlive.current) readerRef.current = handle; else handle.close(); })
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
  const downloadAttachment = async (article: BibArticle, entry: AttachmentEntry) => {
    const key = JSON.stringify([article.id, entry.key]);
    if (downloadLocks.current.has(key)) return;
    downloadLocks.current.add(key);
    setDownloadingAttachments(new Set(downloadLocks.current));
    setError(null);
    try {
      await downloadBibAttachment(article.id, entry.filename || entry.label, entry.attachmentKey, entry.expectedHash);
    } catch (cause) {
      setError(t("main.downloadFailed", { error: String(cause) }));
    } finally {
      downloadLocks.current.delete(key);
      setDownloadingAttachments(new Set(downloadLocks.current));
    }
  };
  const changeAttachment = async (article: BibArticle, entry: AttachmentEntry, remove: boolean) => {
    if (!entry.attachmentKey) return;
    if (remove && !window.confirm(t("main.deleteAttachmentConfirm", { label: entry.label }))) return;
    setBusy("attachment"); setError(null);
    try {
      await gatewayRequest(remove ? "DELETE" : "PUT", `/bib/articles/${encodeURIComponent(article.id)}/attachments/${encodeURIComponent(entry.attachmentKey)}${remove ? "" : "/preferred"}`);
      if (selectionRef.current === article.id) selectArticle(article.id);
      invalidateMetadata(); void loadLibrary(query);
    } catch (failure) { setError(String(failure)); }
    finally { setBusy(null); }
  };
  const withReader = (entry: AttachmentEntry): AttachmentEntry =>
    entry.kind === "pdf" || entry.kind === "text"
      ? { ...entry, onClick: () => readFulltext(entry.format, entry.attachmentKey),
          onDownload: selected ? () => void downloadAttachment(selected, entry) : undefined,
          onRemove: selected && entry.attachmentKey ? () => void changeAttachment(selected, entry, true) : undefined,
          onPrefer: selected && entry.attachmentKey ? () => void changeAttachment(selected, entry, false) : undefined,
          mutating: busy === "attachment",
          downloading: selected ? downloadingAttachments.has(JSON.stringify([selected.id, entry.key])) : false }
      : entry;

  const patchSummary = (articleId: string, patch: (summary: AttachmentSummary) => AttachmentSummary) => {
    setAttachmentSummaries((current) => ({
      ...current,
      [articleId]: patch(current[articleId] ?? emptyAttachmentSummary),
    }));
  };

  const uploadFilesToArticle = async (article: BibArticle, files: Array<File | string>) => {
    if (!files.length) return;
    setBusy("upload"); setError(null);
    try {
      const failures: string[] = [];
      let hasPreferred = article.id === activeDetail?.article.id ? Boolean(activeDetail.fulltext) : Boolean((await gatewayRequest("GET", `/bib/articles/${encodeURIComponent(article.id)}`)).fulltext);
      for (const file of files) {
        try {
          const response = await uploadBibAttachment(article.id, file, { preferred: !hasPreferred });
          hasPreferred = true;
          const pdf = /pdf/i.test(response?.attachment?.content_type ?? "");
          patchSummary(article.id, summary => ({ ...summary, pdf: summary.pdf || pdf, file: true, count: summary.count + 1 }));
        } catch (failure) { failures.push(`${typeof file === "string" ? file.split(/[\\/]/).pop() : file.name}: ${String(failure)}`); }
      }
      if (files.length && selectionRef.current === article.id) selectArticle(article.id);
      if (failures.length) setError(t("main.uploadFailures", { failures: failures.join("; ") }));
    } catch (cause) { setError(String(cause)); }
    finally { setBusy(null); }
  };

  const uploadForArticle = async (article: BibArticle) => {
    try {
      const files = await pickUploadFiles();
      await uploadFilesToArticle(article, files);
    } catch (cause) { setError(String(cause)); }
  };

  const deleteFulltextFor = async (article: BibArticle) => {
    if (!window.confirm(t("main.deleteFulltextConfirm", { title: article.title }))) return;
    setBusy("delete-attachment");
    setError(null);
    try {
      await gatewayRequest("DELETE", `/bib/articles/${encodeURIComponent(article.id)}/fulltext`);
      if (selectionRef.current === article.id) selectArticle(article.id);
      invalidateMetadata();
      await loadLibrary(query);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(null);
    }
  };

  const deleteArticleFor = async (article: BibArticle) => {
    if (!window.confirm(t("main.deleteArticleConfirm", { title: article.title }))) return;
    setBusy("delete-article");
    setError(null);
    readerRef.current?.close();
    readerRef.current = null;
    try {
      await gatewayRequest("DELETE", `/bib/articles/${encodeURIComponent(article.id)}`);
      if (selectionRef.current === article.id) selectArticle(null);
      setCheckedIds(current => current.filter(id => id !== article.id));
      invalidateMetadata();
      await loadLibrary(query);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div ref={libraryContainer} className={cn("relative isolate grid min-h-0 min-w-0 flex-1 grid-rows-[minmax(0,1fr)] overflow-hidden", roomy && (externalSidebar ? "grid-cols-[minmax(0,1fr)_336px] grid-rows-1" : "grid-cols-[216px_minmax(0,1fr)_336px] grid-rows-1"))}>
      {transferMode && <LibraryTransferDialog mode={transferMode} articleIds={exportIds} collectionId={selectedCollection === "@unfiled" ? null : selectedCollection} onClose={() => setTransferMode(null)} onImported={() => { invalidateMetadata(); void loadLibrary(query); }} />}
      {duplicatesOpen && <DuplicateReview onClose={() => setDuplicatesOpen(false)} onMerged={() => { selectArticle(null); setCheckedIds([]); invalidateMetadata(); void loadLibrary(query); }} />}
      {organizingIds && <BatchOrganize ids={organizingIds} collections={collections} onClose={() => setOrganizingIds(null)} onChanged={() => { if (selectionRef.current && organizingIds.includes(selectionRef.current)) selectArticle(selectionRef.current); invalidateMetadata(); void loadLibrary(query); }} />}
      {trashOpen && <ArticleTrash onClose={() => setTrashOpen(false)} onChanged={() => { invalidateMetadata(); void loadLibrary(query); }} />}
      {(editingArticle || creatingArticle) && <MetadataEditor article={editingArticle ?? {id:"", title:"", authors:[], identifiers:[], keywords:[], pub_types:["Journal Article"], source:"manual"}} revision={editingRevision} create={creatingArticle} onClose={() => { setEditingArticle(null); setCreatingArticle(false); }} onSaved={async (article, revision) => {
        setArticles(current => current.map(value => value.id === article.id ? article : value));
        setDetail(current => current?.article.id === article.id ? { ...current, article, revision } : current);
        setEditingArticle(null);
        setCreatingArticle(false);
        let membershipFailure: string | null = null;
        if (creatingArticle) {
          if (selectedCollection && selectedCollection !== "@unfiled") {
            try { await gatewayRequest("POST", `/bib/collections/${encodeURIComponent(selectedCollection)}/articles`, { article_id: article.id }); }
            catch (failure) { membershipFailure = t("main.membershipFailure", { error: String(failure) }); }
          }
          selectArticle(article.id);
        }
        invalidateMetadata();
        await loadLibrary(query);
        if (membershipFailure) setError(membershipFailure);
      }} />}
      {advancedOpen && <AdvancedSearchBuilder query={queryDraft} expression={queryDraft === query ? expression : null} onClose={() => setAdvancedOpen(false)} onApply={(expr,text) => {setExpression(expr);setQuery(text);setQueryDraft(text);setAdvancedOpen(false);}} />}
      {((!externalSidebar && libraryOpen) || detailsOpen) && (
        <button
          type="button"
          aria-label={t("main.closePanel")}
          onClick={() => {
            setLibraryOpen(false);
            setDetailsOpen(false);
          }}
          className={cn("absolute inset-0 z-30 bg-[var(--scrim)] outline-none", roomy && "hidden")}
        />
      )}
      {externalSidebar && sidebarSlot && createPortal(
        <ScrollArea className="min-h-0 flex-1" contentClassName="p-1.5">
          <CollectionTree collections={collections} selected={selectedCollection} total={libraryTotal} onSelect={selectCollection} onAction={collectionAction} />
          <SavedSearches request={draftRequest} scopeLabel={scopeLabel} activeId={activeSavedId} disabled={indexEngine === "legacy" || (mode === "hybrid" && !topicDraft.trim())} onSelect={(search) => { onOpenSidebar?.(); applySavedSearch(search); }} onSaved={applySavedSearch} onDeleted={id => {if (id === activeSavedId) setActiveSavedId(null);}} />
          <SearchIndexStatus onEngine={setIndexEngine} onSourceChange={() => {if (!loading && articleOffset <= 50) latestReload.current();else setUpdatesPending(true);}} />
          <label className="mt-3 flex items-start gap-2 border-t px-2 py-3 text-xs text-muted-foreground"><input type="checkbox" checked={includeDescendants} onChange={e => setIncludeDescendants(e.target.checked)} />{t("main.showSubcollections")}</label>
        </ScrollArea>, sidebarSlot,
      )}
      {!externalSidebar && <aside
        className={cn(
          "relative flex min-h-0 flex-col overflow-hidden border-r bg-card",
          !roomy && "absolute top-0 bottom-0 left-0 z-[35] w-[min(88%,300px)] shadow-2xl transition-transform",
          !roomy && !libraryOpen && "-translate-x-full",
        )}
      >
        <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b px-3">
          <span className="text-xs font-semibold tracking-wide uppercase">{t("main.library")}</span>
          <div className="flex shrink-0 items-center gap-1">
            <Badge variant="secondary" aria-label={t("main.articlesInLibraryAria")} title={t("main.articlesInLibraryTitle")}>{libraryTotal.toLocaleString()}</Badge>
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label={t("main.closeLibrary")}
              className={cn(roomy && "hidden")}
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
          <label className="mt-3 flex items-start gap-2 border-t px-2 py-3 text-xs text-muted-foreground"><input type="checkbox" checked={includeDescendants} onChange={e => setIncludeDescendants(e.target.checked)} />{t("main.showSubcollections")}</label>
        </ScrollArea>
      </aside>}

      <section className="flex min-h-0 flex-col overflow-hidden bg-card">
        <div className="flex h-10 shrink-0 items-center gap-2 border-b px-2">
          <Button
            size="icon-sm"
            variant="ghost"
            aria-label={t("main.openLibrary")}
            className={cn(!externalSidebar && roomy && "hidden")}
            onClick={() => externalSidebar ? onOpenSidebar?.() : setLibraryOpen(true)}
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
              aria-label={t("main.searchBibliographyAria")}
              value={mode === "hybrid" ? topicDraft : queryDraft}
              onCompositionStart={() => { composing.current = true; setIsComposing(true); }}
              onCompositionEnd={(event) => { composing.current = false; setIsComposing(false); if (mode === "hybrid") setTopicDraft(event.currentTarget.value); else setQueryDraft(event.currentTarget.value); }}
              placeholder={mode === "hybrid" ? t("main.topicPlaceholder") : t("main.searchPlaceholder", { count: total.toLocaleString() })}
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
          {mode === "hybrid" && <Button size="xs" variant="secondary" disabled={!topicDraft.trim() || isComposing} onClick={commitMainQuery}>{t("main.searchTopic")}</Button>}
          {mode === "hybrid" && loading && <Button size="xs" variant="ghost" aria-label={t("main.cancelSearchAria")} onClick={() => {requestSequence.current++;listAbort.current?.abort();setLoading(false);setError(null);setSearchCancelled(true);}}>{t("main.cancel")}</Button>}
          <Button size="xs" variant="ghost" disabled={indexEngine === "legacy"} onClick={() => setAdvancedOpen(true)}>{t("main.advanced")}</Button>
          <Button size="icon-sm" variant="ghost" aria-label={t("main.newArticle")} title={t("main.newArticle")} onClick={() => { setEditingRevision(undefined); setCreatingArticle(true); }}><Plus /></Button>
          <Button size="icon-sm" variant="ghost" aria-label={t("main.articleTrash")} title={t("main.articleTrash")} onClick={() => setTrashOpen(true)}><Trash2 /></Button>
          <Button size="icon-sm" variant="ghost" aria-label={t("main.importArticle")} title={t("main.importArticle")} onClick={() => void importArticle()} disabled={busy === "import"}>
            <Link2 />
          </Button>
          <Button
            size="icon-sm"
            variant="ghost"
            aria-label={t("main.openEntryDetails")}
            className={cn(roomy && "hidden")}
            onClick={() => setDetailsOpen(true)}
          >
            <PanelRight />
          </Button>
          <Button size="icon-sm" variant="ghost" aria-label={t("main.exportBibtexAria")} title={retrieval ? t("main.exportCandidatesTitle") : t("main.exportBibtexAria")} onClick={() => void exportLibrary()} disabled={loading || busy === "export" || (mode === "hybrid" && resultContext !== JSON.stringify(currentRequest))}>
            <Download />
          </Button>
        </div>

        <div className="flex shrink-0 flex-wrap items-center gap-1 border-b px-2 py-1" aria-label={t("main.libraryActionsAria")}>
          <BrowserConnectorHelp />
          <ZoteroBundleImport onImported={() => { invalidateMetadata(); void loadLibrary(query); }} />
          <Button size="xs" variant="ghost" onClick={() => setTransferMode("import")}><Upload />{t("main.importFile")}</Button>
          <Button size="xs" variant="ghost" disabled={!checkedIds.length && !selected} title={t("main.exportCheckedTitle")} onClick={() => { setExportIds(checkedIds.length ? [...checkedIds] : selected ? [selected.id] : []); setTransferMode("export"); }}><Quote />{t("main.exportAndCite")}</Button>
          <Button size="xs" variant="ghost" onClick={() => setDuplicatesOpen(true)}><Copy />{t("main.findDuplicates")}</Button>
          {checkedIds.length > 0 && <Button size="xs" variant="secondary" onClick={() => setOrganizingIds([...checkedIds])}>{t("main.organize", { count: checkedIds.length })}</Button>}
        </div>
        <div className="flex flex-wrap items-center gap-2 border-b px-2 py-1.5 text-xs">
          <select aria-label={t("main.searchModeAria")} className="rounded border bg-background p-0.5" disabled={indexEngine === "legacy"} value={mode} onChange={event => {setLocationError("");setMode(event.target.value as SearchMode);}}><option value="lexical">{t("main.modeKeywords")}</option><option value="hybrid">{t("main.modeTopic")}</option></select>
          <span className="min-w-0 truncate text-muted-foreground">{scopeLabel}{selectedCollection && includeDescendants ? t("main.includingSubcollections") : ""}</span>
          <select aria-label={t("main.quickFieldsAria")} title={t("main.quickFieldsTitle")} className="max-w-44 rounded border bg-background p-0.5" disabled={indexEngine === "legacy"} value={Object.keys(quickFields).find(key => JSON.stringify(quickFields[key]) === JSON.stringify(defaultFields)) ?? "custom"} onChange={event => setDefaultFields(quickFields[event.target.value] ?? [])}><option value="all">{t("main.fieldsEverything")}</option><option value="title">{t("main.fieldsTitle")}</option><option value="metadata">{t("main.fieldsMetadata")}</option><option value="custom" disabled>{t("main.fieldsCustom")}</option></select>
          <Button size="xs" variant={filtersOpen ? "secondary" : "ghost"} disabled={indexEngine === "legacy"} aria-expanded={filtersOpen} onClick={() => setFiltersOpen(!filtersOpen)}>{t("main.filters")}</Button>
          <label className="flex items-center gap-1" title={t("main.sameAttachmentTitle")}><input type="checkbox" disabled={indexEngine === "legacy"} checked={sameChild} onChange={event => setSameChild(event.target.checked)} />{t("main.sameAttachment")}</label>
          <SearchAssist topic={mode === "hybrid"} history={history} onClear={clearHistory} onPick={text=>{if (mode === "hybrid") {setTopicDraft(text);setTopic(text);} else {setQueryDraft(text);commitQuery(text);}searchInputRef.current?.focus();}} />
          <ColumnsPicker columns={columns} onChange={setColumns} snippets={showSnippets} onSnippets={setShowSnippets} />
          <span role="status" className="ml-auto text-muted-foreground">{loading ? t("main.searching") : t(retrieval ? "main.candidateCount" : "main.articleCount", { count: total.toLocaleString() })}</span>
          <select aria-label={t("main.sortAria")} disabled={indexEngine === "legacy"} className="rounded border bg-background px-1 py-0.5" value={sort} onChange={event => setSort(event.target.value as SearchSort)}><option value="relevance">{t("main.sortRelevance")}</option><option value="newest">{t("main.sortNewest")}</option><option value="oldest">{t("main.sortOldest")}</option><option value="title">{t("main.sortTitle")}</option></select>
          {searchActive && <Button size="xs" variant="ghost" onClick={clearSearch}>{t("main.clearSearch")}</Button>}
        </div>
        {mode === "hybrid" && <>
          <div className="space-y-1 border-b px-2 py-2 text-xs">
            <label className="flex flex-wrap items-center gap-2">{t("main.requiredConditions")}<Input aria-label={t("main.requiredConditionsAria")} className="h-7 min-w-40 flex-1 text-xs" value={queryDraft} placeholder={t("main.requiredConditionsPlaceholder")} onChange={event => {setLocationError("");setQueryDraft(event.target.value);}} onCompositionStart={() => {composing.current=true;setIsComposing(true);}} onCompositionEnd={event => {composing.current=false;setIsComposing(false);setQueryDraft(event.currentTarget.value);}} onKeyDown={event => {if (event.key === "Enter" && !event.nativeEvent.isComposing && !composing.current) {event.preventDefault();commitQuery();}}} /></label>
            <p className="text-muted-foreground">{t("main.requiredConditionsHint")}</p>
            {(!topic.trim() || topicDraft.trim() !== topic) && <p role="status">{t("main.submitTopicHint")}</p>}
          </div>
          <SemanticSearchControls onInvalidate={() => {setUpdatesPending(true);setNextCursor(null);setHasMoreArticles(false);}} onReady={() => {if (!loading && articleOffset <= 50) latestReload.current();else setUpdatesPending(true);}} onKeyword={() => setMode("lexical")} />
        </>}
        {resultContext && resultContext !== JSON.stringify(currentRequest) && <p role="status" className="border-b px-2 py-1 text-xs text-muted-foreground">{t("main.previousResultsRetained")}</p>}
        {mode === "hybrid" && searchCancelled && <p role="status" className="border-b px-2 py-1 text-xs text-muted-foreground">{t("main.searchCancelled")}</p>}
        {retrieval && <p className="border-b px-2 py-1 text-xs text-muted-foreground">{t("main.candidatePoolNote", { count: retrieval.eligible_articles.toLocaleString() })}</p>}
        {filtersOpen && <SearchFiltersPanel value={filters} facets={facets} onChange={setFilters} />}
        {updatesPending && <div className="flex items-center justify-between border-b px-2 py-1 text-xs">{t("main.indexUpdated")}<Button size="xs" variant="ghost" onClick={() => void loadLibrary(query)}>{t("main.refreshResults")}</Button></div>}
        {facets && !filtersOpen && <div className="flex flex-wrap items-center gap-2 border-b px-2 py-1.5 text-xs">
          <select aria-label={t("main.tagFilterAria")} value={filters.tag_ids?.[0] ?? ""} onChange={event => setFilters(current => ({...current,tag_ids:event.target.value ? [Number(event.target.value)] : []}))} className="max-w-40 rounded border bg-background p-1"><option value="">{t("main.allTags")}</option>{facets.tags.map(value => <option key={value.value} value={value.value}>{value.label} ({value.count})</option>)}</select>
          <select aria-label={t("main.creatorFilterAria")} value={filters.creator_ids?.[0] ?? ""} onChange={event => setFilters(current => ({...current,creator_ids:event.target.value ? [Number(event.target.value)] : []}))} className="max-w-40 rounded border bg-background p-1"><option value="">{t("main.allAuthors")}</option>{facets.creators.map(value => <option key={value.value} value={value.value}>{value.label} ({value.count})</option>)}</select>
          <label className="flex items-center gap-1"><input type="checkbox" checked={filters.has_fulltext === true} onChange={event => setFilters(current => ({...current,has_fulltext:event.target.checked ? true : null}))}/>{t("searchFilters.hasFullTextLabel")}</label>
          <label className="flex items-center gap-1"><input type="checkbox" checked={filters.has_annotations === true} onChange={event => setFilters(current => ({...current,has_annotations:event.target.checked ? true : null}))}/>{t("searchFilters.hasNotesLabel")}</label>
        </div>}
        {error && <div role="alert" className="border-b border-destructive/30 bg-destructive/10 p-2 text-xs text-destructive">{error}<Button size="xs" variant="ghost" onClick={() => void loadLibrary(query)}>{t("main.retry")}</Button></div>}
        {/* Mirror the row structure (24px checkbox track outside the row's
            px-2 padding) so the header checkbox lands on the row checkboxes. */}
        <div className="grid h-8 shrink-0 grid-cols-[24px_minmax(0,1fr)] items-center border-b bg-muted/60 text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
          <span className="flex items-center justify-center">
            <Checkbox
              aria-label={t("main.selectAllAria")}
              checked={allVisibleChecked}
              indeterminate={!allVisibleChecked && someVisibleChecked}
              onCheckedChange={(checked) => toggleAllChecked(checked === true)}
            />
          </span>
          <div style={{gridTemplateColumns:buttonColumns}} className="grid h-8 items-center px-2">
            <span />
            <span>{t("main.titleColumn")}{checkedIds.length ? t("main.selectedCount", { count: checkedIds.length }) : ""}</span>
            {wideColumns && columns.creator && <span>{t("main.creatorColumn")}</span>}
            {columns.year && <span>{t("main.yearColumn")}</span>}
            {wideColumns && columns.journal && <span>{t("main.journalColumn")}</span>}
            {columns.files && <span className="text-center">{t("main.filesColumn")}</span>}
          </div>
        </div>

        <ScrollArea className="min-h-0 flex-1" contentClassName="pb-6" viewportRef={resultsViewport} viewportLabel={t("virtualList.viewport")} viewportTabIndex={0} viewportClassName="focus-visible:ring-1 focus-visible:ring-ring" tabIndex={-1}>
          {loading && !visibleArticles.length && (
            <div className="space-y-px">
              {Array.from({ length: 10 }).map((_, index) => (
                <div key={index} className="h-9 animate-pulse border-b bg-muted/30" />
              ))}
            </div>
          )}

          <VirtualArticleList articles={visibleArticles} total={total} viewport={resultsViewport} contextKey={JSON.stringify({request:currentRequest,snippets:showSnippets})} estimatedHeight={showSnippets && Boolean(query.trim() || expression) ? 76 : 37} selectedId={selectedArticle} revealId={revealId} onNavigate={selectArticle} onToggle={id => toggleChecked(id, !checkedIds.includes(id))}>{(article) => {
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
                        aria-label={t("main.selectArticleAria", { title: article.title })}
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
                      {wideColumns && columns.creator && <span className="min-w-0 truncate pr-3 text-[13px] text-muted-foreground">{creatorDisplay(article, t("main.unknown"), surname => t("main.etAl", { surname }))}</span>}
                      {columns.year && <span className="text-xs text-muted-foreground">{article.year ?? "—"}</span>}
                      {wideColumns && columns.journal && <span className="min-w-0 truncate pr-3 text-xs text-muted-foreground">{article.journal ?? "—"}</span>}
                      {columns.files && <span className="flex items-center justify-center text-muted-foreground"><AttachmentTypeIcons summary={summary} /></span>}
                    </button>
                  </div>

                  {showSnippets && Boolean(query.trim() || expression || retrieval) && searchHits[article.id] && <div className="px-9 pb-2 text-xs text-muted-foreground">
                    <RecallLabel recall={retrieval?.hits[article.id]} />
                    <span className="mr-2 font-medium">{searchHits[article.id].matched_field}</span>
                    <SearchSnippet text={searchHits[article.id].snippet} ranges={searchHits[article.id].highlight_ranges} />
                    <Button size="xs" variant="ghost" onClick={() => selectArticle(article.id)}>{t("main.viewEvidence")}</Button>
                  </div>}
                  {isSelected && attachmentsOpen && (
                    <div className="bg-muted/25">
                      {detailLoading && (
                        <div className="px-7 py-2 text-xs text-muted-foreground">{t("main.loadingAttachments")}</div>
                      )}
                      {!detailLoading && !attachments.length && (
                        <div className="px-7 py-2 text-xs text-muted-foreground">{t("main.noAttachments")}</div>
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
                    {t("main.read")}
                  </ContextMenuItem>
                  <ContextMenuItem disabled={busy === "upload"} onClick={() => void uploadForArticle(article)}>
                    <Upload />
                    {t("main.uploadAttachment")}
                  </ContextMenuItem>
                  <ContextMenuItem
                    variant="destructive"
                    disabled={!summary.pdf || busy === "delete-attachment"}
                    onClick={() => void deleteFulltextFor(article)}
                  >
                    <FileX2 />
                    {t("main.deleteAttachment")}
                  </ContextMenuItem>
                  <ContextMenuSeparator />
                  <ContextMenuItem
                    variant="destructive"
                    disabled={busy === "delete-article"}
                    onClick={() => void deleteArticleFor(article)}
                  >
                    <Trash2 />
                    {t("main.deleteArticle")}
                  </ContextMenuItem>
                </ContextMenuContent>
              </ContextMenu>
            );
          }}</VirtualArticleList>

          {!loading && !visibleArticles.length && (
            <div className="grid h-full place-items-center text-sm text-muted-foreground">{error ? t("main.searchFailed") : t("main.noMatchingArticles")}</div>
          )}
          {!loading && hasMoreArticles && (
            <div ref={articleLoaderRef} className="p-3 text-center">
              <Button size="sm" variant="secondary" onClick={loadMoreArticles}>
                {t("main.loadMore", { loaded: articleOffset.toLocaleString(), total: total.toLocaleString() })}
              </Button>
            </div>
          )}
        </ScrollArea>
      </section>

      <aside
        className={cn(
          "relative flex min-h-0 flex-col overflow-hidden border-l bg-card",
          !roomy && "absolute top-0 bottom-0 right-0 z-[35] w-[min(94%,360px)] shadow-2xl transition-transform",
          !roomy && !detailsOpen && "translate-x-full",
        )}
        onDragOver={(event) => {
          if (!selected || !Array.from(event.dataTransfer.types).includes("Files")) return;
          event.preventDefault();
          setDetailDropActive(true);
        }}
        onDragLeave={(event) => {
          if (event.currentTarget.contains(event.relatedTarget as Node | null)) return;
          setDetailDropActive(false);
        }}
        onDrop={(event) => {
          setDetailDropActive(false);
          if (!selected || busy === "upload" || !event.dataTransfer.files.length) return;
          event.preventDefault();
          void uploadFilesToArticle(selected, Array.from(event.dataTransfer.files));
        }}
      >
        <div className="flex h-9 shrink-0 items-center justify-between gap-2 border-b px-3">
          <span className="min-w-0 truncate text-[13px] font-medium">
            {selected ? t("main.entryDetails") : t("main.bibliography")}
          </span>
          <div className="flex shrink-0 items-center gap-1">
            {selected && <Button size="icon-xs" variant="ghost" aria-label={t("main.editMetadata")} title={t("main.editMetadata")}
              disabled={detailLoading || !activeDetail} onClick={() => { if (activeDetail) { setEditingRevision(activeDetail.revision); setEditingArticle(activeDetail.article); } }}><Pencil /></Button>}
            {selected && <Button size="icon-xs" variant="ghost" aria-label={t("main.uploadAttachment")} title={t("main.uploadAttachment")}
              disabled={busy === "upload"} onClick={() => void uploadForArticle(selected)}><Upload /></Button>}
            {detailLoading && <Badge variant="secondary">{t("main.loading")}</Badge>}
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label={t("main.closeEntryDetails")}
              className={cn(roomy && "hidden")}
              onClick={() => setDetailsOpen(false)}
            >
              <X />
            </Button>
          </div>
        </div>

        {detailDropActive && selected && (
          <div className="pointer-events-none absolute inset-0 z-[40] m-2 flex items-center justify-center rounded-md border-2 border-dashed border-primary/60 bg-background/70">
            <span className="rounded bg-background px-2 py-1 text-xs font-medium text-primary">{t("main.dropToAttach")}</span>
          </div>
        )}

        <ScrollArea className="min-h-0 flex-1" contentClassName="p-3">
          {error && (
            <div className="mb-3 rounded-md border border-destructive/40 bg-destructive/10 p-2 text-xs text-destructive">
              {error}
            </div>
          )}

          {!selected ? (
            <div className="grid h-full place-items-center text-sm text-muted-foreground">{t("main.selectAnItem")}</div>
          ) : (
            <div className="space-y-3">
              <AddToContext resource={articleContext(selected)} />
              {searchActive && indexGeneration && resultContext===JSON.stringify(currentRequest) && articles.some(article=>article.id===selected.id) && <SearchEvidencePanel articleId={selected.id} request={currentRequest} generation={indexGeneration} onOpen={evidence => void openEvidence(evidence)} />}
              <Section title={t("main.attachmentInfo")} count={attachments.length} action={
                <Button size="xs" variant="ghost" className="h-6 gap-1 px-1.5 text-[11px]" disabled={busy === "upload"}
                  onClick={() => void uploadForArticle(selected)}>
                  <Upload className="size-3" />{t("main.upload")}
                </Button>
              }>
                {!detailLoading && attachments.length ? (
                  <div className="space-y-0.5">
                    {attachments.map((entry) => (
                      <AttachmentRow key={entry.key} entry={withReader(entry)} />
                    ))}
                  </div>
                ) : (
                  <div className="text-xs text-muted-foreground">
                    {detailLoading ? t("main.loading") : (
                      <button type="button" className="w-full rounded-md border border-dashed border-border px-2 py-3 text-center text-xs text-muted-foreground transition-colors hover:border-primary/50 hover:text-foreground disabled:opacity-50"
                        disabled={busy === "upload"} onClick={() => void uploadForArticle(selected)}>
                        {busy === "upload" ? t("main.uploading") : t("main.noAttachmentUpload")}
                      </button>
                    )}
                  </div>
                )}
              </Section>

              <LiteratureExploration article={selected} importTarget={selectedCollection && selectedCollection !== "@unfiled" ? { collection_id: selectedCollection } : { library_id: libraryId ?? 1 }} targetLabel={scopeLabel} onImported={() => { invalidateMetadata(); void loadLibrary(query); }} />

              <Section title={t("main.detailsSection")}>
                <MetadataTable>
                  <MetadataRow label={t("main.f_title")} value={selected.title} />
                  {(selected.authors ?? []).map((author, index) => (
                    <MetadataRow
                      key={`${author.name ?? author.last_name ?? author.fore_name ?? "author"}-${index}`}
                      label={index === 0 ? t("main.f_authors") : ""}
                      value={authorName(author)}
                    />
                  ))}
                  <MetadataRow label={t("main.f_itemType")} value={selected.pub_types?.join(", ")} />
                  <MetadataRow label={t("main.f_publication")} value={selected.journal} />
                  <MetadataRow label={t("main.f_volume")} value={selected.volume} />
                  <MetadataRow label={t("main.f_issue")} value={selected.issue} />
                  <MetadataRow label={t("main.f_pages")} value={selected.pages} />
                  <MetadataRow label={t("main.f_language")} value={selected.language} />
                  <MetadataRow label={t("main.f_issn")} value={selected.issn} />
                  <MetadataRow label={t("main.f_essn")} value={selected.essn} />
                  <MetadataRow label={t("main.f_date")} value={selected.year} />
                  <MetadataRow label={t("main.f_source")} value={selected.source} />
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
                  {(selected.identifiers ?? []).filter(identifier => identifier.kind === "other" && /^https?:\/\//i.test(identifier.value)).map(identifier => <MetadataRow key={identifier.value} label={t("main.f_url")} value={identifier.value} href={identifier.value} />)}
                  {Object.entries(selected.extra_fields ?? {}).map(([key, value]) => <MetadataRow key={`extra-${key}`} label={t(`metadataEditor.extraFields.${key}`, { defaultValue: extraFieldLabels[key] ?? key })} value={value} href={key === "url" && /^https?:\/\//i.test(value) ? value : null} />)}
                  <MetadataRow label={t("main.f_abstract")} value={selected.abstract_text} />
                  <MetadataRow label={t("main.f_record")} value={selected.id} />
                </MetadataTable>
              </Section>
              <DiscoveryProvenance key={selected.id} articleId={selected.id} />

              <Section title={t("main.collectionsSection")} count={associations.filter(a => a.article_id === selected.id).length}>
                <div className="space-y-2">
                  {associations.filter(a => a.article_id === selected.id).map(a => <div key={a.collection_id} className="flex items-center gap-1 text-xs"><button className="min-w-0 flex-1 truncate text-left hover:underline" onClick={() => selectCollection(a.collection_id)}>{collectionPath(collections, a.collection_id)}</button><Button size="icon-xs" variant="ghost" aria-label={t("main.removeFromCollectionAria", { name: collectionPath(collections, a.collection_id) })} onClick={() => void updateMembership(a.collection_id, selected.id, true)}><X /></Button></div>)}
                  <select aria-label={t("main.addToCollectionAria")} value="" className="w-full rounded border bg-background p-1.5 text-xs" onChange={e => { if (e.target.value) void updateMembership(e.target.value, selected.id); }}><option value="">{t("main.addToCollection")}</option>{collections.filter(c => !c.deleted && !associations.some(a => a.article_id === selected.id && a.collection_id === c.id)).map(c => <option key={c.id} value={c.id}>{collectionPath(collections, c.id)}</option>)}</select>
                </div>
              </Section>

              <Section title={t("main.tagsSection")} count={selected.keywords?.length ?? 0}>
                {selected.keywords?.length ? (
                  <div className="flex flex-wrap gap-1">
                    {selected.keywords.map((keyword) => (
                      <Badge key={keyword} variant="outline" className="max-w-full">
                        <span className="truncate">{keyword}</span>
                      </Badge>
                    ))}
                  </div>
                ) : (
                  <div className="text-xs text-muted-foreground">{t("main.noTags")}</div>
                )}
              </Section>

              <Section title={t("main.notesLabel")} count={activeDetail?.annotations.length ?? 0}>
                {activeDetail?.annotations.length ? (
                  <div className="space-y-2">
                    {activeDetail.annotations.map((note) => (
                      <article id={`bib-note-${note.id}`} tabIndex={-1} key={note.id} className="rounded-md border p-2">
                        <div className="mb-1 flex items-center gap-1.5 text-xs text-muted-foreground">
                          <StickyNote className="size-3" />
                          {note.kind || t("main.note")}
                        </div>
                        <p className="text-[13px] leading-5 break-words whitespace-pre-wrap">{note.content}</p>
                      </article>
                    ))}
                  </div>
                ) : (
                  <div className="text-xs text-muted-foreground">{t("main.noNotes")}</div>
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
              {activeDetail?.fulltext?.file_format?.toLowerCase() === "txt" ? t("main.readTxt") : t("main.readPdf")}
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
