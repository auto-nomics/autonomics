import { useCallback, useEffect, useRef, useState } from "react";
import { WikiPanel } from "@/components/wiki/wiki-panel";
import { toast } from "sonner";

import { ArticleDetailPanel } from "@/components/bibliography/article-detail";
import { CollectionTree, type CollectionView } from "@/components/bibliography/collection-tree";
import { RecordList } from "@/components/bibliography/record-list";
import {
  api,
  articleIdentifier,
  reextractFulltext,
  type Article,
  type ArticleDetail,
  type BibHealth,
  type Collection,
} from "@/lib/bib-api";


export function App() {
  const [topView, setTopView] = useState<"bib" | "wiki">(() => {
    try {
      return localStorage.getItem("app.topView") === "wiki" ? "wiki" : "bib";
    } catch {
      return "bib";
    }
  });
  const [health, setHealth] = useState<BibHealth | null>(null);
  const [recursive,setRecursive]=useState(()=>{try{return localStorage.getItem("bib.collections.recursive")==="true";}catch{return false;}});
  const recursiveRef=useRef(recursive);
  const recordSequence=useRef(0);
  const [collections, setCollections] = useState<Collection[]>([]);
  const [activeView, setActiveView] = useState<CollectionView>("all");
  const [records, setRecords] = useState<Article[]>([]);
  const [recordsLoading, setRecordsLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<ArticleDetail | null>(null);

  const [importType, setImportType] = useState("doi");
  const [importId, setImportId] = useState("");
  const [importing, setImporting] = useState(false);

  const loadHealth = useCallback(async () => {
    const data = await api<BibHealth>("/api/v1/bib/health");
    setHealth(data);
  }, []);

  const loadCollections = useCallback(async () => {
    const data = await api<{ collections: Collection[] }>("/api/v1/bib/collections?include_deleted=true");
    setCollections(data.collections);
    return data.collections;
  }, []);

  const selectArticle = useCallback(async (id: string) => {
    const data = await api<ArticleDetail>(`/api/v1/bib/articles/${encodeURIComponent(id)}`);
    setSelected(data);
  }, []);

  // Background extraction runs after uploads and re-extract requests.
  // Poll ~every 2.5 s until a terminal state (10 min budget, matching the
  // server-side deadline). Refreshes the open detail panel without
  // clobbering a different article the user may have selected meanwhile.
  const pollExtraction = useCallback(async (articleId: string) => {
    for (let attempt = 0; attempt < 240; attempt++) {
      await new Promise((resolve) => setTimeout(resolve, 2500));
      try {
        const data = await api<ArticleDetail>(
          `/api/v1/bib/articles/${encodeURIComponent(articleId)}`,
        );
        setSelected((current) =>
          current && current.article.id === articleId ? data : current,
        );
        const status = data.fulltext?.extract_status;
        if (status === "done") {
          toast.success("Full text extraction finished");
          return;
        }
        if (status === "failed") {
          toast.error(`Extraction failed: ${data.fulltext?.extract_error ?? "unknown error"}`);
          return;
        }
      } catch {
        // Transient fetch failure — keep polling within the attempt budget.
      }
    }
    toast.info("Extraction still running — reopen the article later to see the result");
  }, []);

  const loadRecords = useCallback(
    async (view: CollectionView, searchTerm: string) => {
      const sequence=++recordSequence.current;
      setRecordsLoading(true);
      try {
        let nextRecords: Article[] = [];

        const endpoint=view==="all"?"/api/v1/bib/articles":view==="unfiled"?"/api/v1/bib/articles/unfiled":`/api/v1/bib/collections/${encodeURIComponent(view.slice(4))}/articles`;
        let offset=0;
        for (;;) {
          const data=await api<{articles:Article[];total:number}>(`${endpoint}?query=${encodeURIComponent(searchTerm.trim())}&include_descendants=${recursiveRef.current}&limit=500&offset=${offset}`);
          nextRecords.push(...data.articles);offset+=data.articles.length;
          if(!data.articles.length || data.articles.length<500 || (typeof data.total==="number" && offset>=data.total))break;
        }

        if(sequence===recordSequence.current)setRecords(nextRecords);
      } finally {
        if(sequence===recordSequence.current)setRecordsLoading(false);
      }
    },
    [],
  );

  const latestCollectionReload=useRef<()=>Promise<void>>(async()=>{});
  latestCollectionReload.current=async()=>{
    const next=await loadCollections();
    const view=activeView.startsWith("col:") && !next.some(c=>!c.deleted && c.id===activeView.slice(4)) ? "all" : activeView;
    if(view!==activeView){setActiveView(view);setSelected(null);}
    await Promise.all([loadHealth(),loadRecords(view,query)]);
  };
  useEffect(()=>{
    let stopped=false;let revision:number|undefined;
    void(async()=>{while(!stopped){try{
      const next=await api<{revision:number}>(`/api/v1/bib/collections/changes${revision===undefined?"":`?since=${revision}`}`);
      if(stopped)return;
      if(typeof next.revision!=="number")throw new Error("Missing revision");
      if(revision!==undefined && revision!==next.revision)await latestCollectionReload.current();
      revision=next.revision;
    }catch{if(!stopped)await new Promise(resolve=>setTimeout(resolve,2000));}}})();
    return ()=>{stopped=true;};
  },[]);

  useEffect(() => {
    void (async () => {
      await loadCollections();
      await Promise.all([loadHealth(), loadRecords("all", "")]);
    })().catch((error: Error) => toast.error(error.message));
  }, [loadCollections, loadHealth, loadRecords]);

  async function changeView(view: CollectionView) {
    setActiveView(view);
    setSelected(null);
    await loadRecords(view, query).catch((error: Error) => toast.error(error.message));
  }

  async function searchRecords() {
    await loadRecords(activeView, query).catch((error: Error) =>
      toast.error(error.message),
    );
  }

  async function importArticle(idType: string, id: string) {
    setImporting(true);
    try {
      const data = await api<{ article: Article; cached: boolean }>(
        "/api/v1/bib/articles/import",
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ id_type: idType, id: id.trim(), fetch_fulltext: true, collection_id:activeView.startsWith("col:")?activeView.slice(4):null }),
        },
      );


      await loadCollections();
      await Promise.all([loadHealth(), loadRecords(activeView, query), selectArticle(data.article.id)]);
      setImportId("");
      toast.success(data.cached ? "Article already in library" : "Article imported");
    } catch (error) {
      toast.error((error as Error).message);
    } finally {
      setImporting(false);
    }
  }

  async function uploadFulltext(file: File) {
    if (!selected) return;
    const articleId = selected.article.id;
    const form = new FormData();
    form.append("file", file);
    const response = await fetch(
      `/api/v1/bib/articles/${encodeURIComponent(articleId)}/fulltext`,
      { method: "POST", body: form },
    );
    const data = await response.json().catch(() => null);
    if (!response.ok) throw new Error(data?.error ?? response.statusText);
    await selectArticle(articleId);
    toast.success("Uploaded — extraction started");
    void pollExtraction(articleId);
  }

  async function reextract() {
    if (!selected) return;
    const articleId = selected.article.id;
    await reextractFulltext(articleId);
    await selectArticle(articleId);
    toast.success("Re-extraction started");
    void pollExtraction(articleId);
  }

  async function addNote(kind: string, content: string) {
    if (!selected) return;
    await api(`/api/v1/bib/articles/${encodeURIComponent(selected.article.id)}/annotations`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ kind, content }),
    });
    await selectArticle(selected.article.id);
    toast.success("Note added");
  }

  async function addToCollection(collectionId: string) {
    if (!selected) return;
    await api(`/api/v1/bib/collections/${encodeURIComponent(collectionId)}/articles`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ article_id: selected.article.id }),
    });
    await loadCollections();
    await loadRecords(activeView, query);
    await selectArticle(selected.article.id);
    toast.success("Added to collection");
  }

  async function removeFromCollection(collectionId: string, articleId: string) {
    await api(
      `/api/v1/bib/collections/${encodeURIComponent(collectionId)}/articles/${encodeURIComponent(articleId)}`,
      { method: "DELETE" },
    );
    await loadCollections();
    await Promise.all([loadRecords(activeView, query), selectArticle(articleId)]);
    toast.success("Removed from collection");
  }

  async function createCollection(name: string,parentId:string|null) {
    const data = await api<{ collection: Collection }>("/api/v1/bib/collections", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name,parent_id:parentId }),
    });
    await loadCollections();
    setActiveView(`col:${data.collection.id}`);
    setSelected(null);
    await loadRecords(`col:${data.collection.id}`, "");
    toast.success("Collection created");
  }

  async function deleteCollection(id: string) {
    const result=await api<{deleted_ids:string[]}>(`/api/v1/bib/collections/${encodeURIComponent(id)}`, { method: "DELETE" });
    const nextView: CollectionView = activeView.startsWith("col:") && result.deleted_ids.includes(activeView.slice(4)) ? "all" : activeView;
    await loadCollections();
    setActiveView(nextView);
    await loadRecords(nextView, query);
    toast.success("Collection deleted");
  }

  async function manageCollection(id:string,action:"rename"|"move"|"restore"|"erase",value?:string|null) {
    await api(`/api/v1/bib/collections/${encodeURIComponent(id)}${action==="restore"?"/restore":action==="erase"?"?permanent=true":""}`,{method:action==="restore"?"POST":action==="erase"?"DELETE":"PATCH",headers:{"content-type":"application/json"},...((action==="rename"||action==="move")?{body:JSON.stringify(action==="rename"?{name:value}:{parent_id:value??null})}:{})});
    await loadCollections();await loadRecords(activeView,query);
  }

  async function deleteArticle() {
    if (!selected) return;
    const id = selected.article.id;
    await api(`/api/v1/bib/articles/${encodeURIComponent(id)}`, { method: "DELETE" });
    setSelected(null);
    await loadCollections();
    await Promise.all([loadHealth(), loadRecords(activeView, query)]);
    toast.success("Article deleted");
  }

  const bibPane = (
    <div className="flex h-screen min-h-0 flex-col overflow-y-auto bg-background lg:overflow-hidden">

      <main className="grid w-full min-h-0 flex-1 gap-4 px-4 py-4 lg:grid-rows-[minmax(0,1fr)] lg:items-stretch lg:grid-cols-[260px_minmax(320px,2fr)_minmax(420px,3fr)] lg:px-6 xl:grid-cols-[280px_minmax(360px,2fr)_minmax(480px,3fr)]">
        <aside className="flex min-w-0 flex-col lg:min-h-0 lg:h-full">
          <CollectionTree
            collections={collections}
            activeView={activeView}
            allCount={health?.article_count ?? 0}
            onSelect={(view) => void changeView(view)}
            onCreate={createCollection}
            onManage={manageCollection}
            recursive={recursive}
            onRecursive={value=>{setRecursive(value);recursiveRef.current=value;localStorage.setItem("bib.collections.recursive",String(value));void loadRecords(activeView,query).catch((e:Error)=>toast.error(e.message));}}
            onDelete={deleteCollection}
          />
        </aside>

        <section className="flex min-w-0 flex-col lg:min-h-0 lg:h-full">
          <RecordList
            activeView={activeView}
            collections={collections}
            records={records}
            loading={recordsLoading}
            query={query}
            setQuery={setQuery}
            onSearch={() => void searchRecords()}
            selectedId={selected?.article.id}
            onSelect={(id) => void selectArticle(id).catch((error: Error) => toast.error(error.message))}
            importType={importType}
            setImportType={setImportType}
            importId={importId}
            setImportId={setImportId}
            importing={importing}
            onImportId={() => void importArticle(importType, importId)}
          />
        </section>

        <section className="flex min-w-0 flex-col lg:min-h-0 lg:h-full">
          <ArticleDetailPanel
            detail={selected}
            collections={collections}
            onUpload={(file) => void uploadFulltext(file).catch((error: Error) => toast.error(error.message))}
            onReextract={() => void reextract().catch((error: Error) => toast.error(error.message))}
            onAddNote={(kind, content) => void addNote(kind, content).catch((error: Error) => toast.error(error.message))}
            onAddToCollection={(collectionId) => void addToCollection(collectionId).catch((error: Error) => toast.error(error.message))}
            onRemoveFromCollection={(collectionId, articleId) =>
              void removeFromCollection(collectionId, articleId).catch((error: Error) => toast.error(error.message))
            }
            onDelete={() => void deleteArticle().catch((error: Error) => toast.error(error.message))}
          />
        </section>
      </main>
    </div>
  );

  return (
    <div className="flex h-screen min-h-0 flex-col bg-background">
      <nav className="flex items-center gap-1 border-b px-3 py-1.5 text-sm">
        {(["bib", "wiki"] as const).map((view) => (
          <button
            key={view}
            type="button"
            className={`rounded-md px-3 py-1 transition-colors ${
              topView === view ? "bg-accent font-medium" : "text-muted-foreground hover:bg-accent"
            }`}
            onClick={() => {
              setTopView(view);
              try {
                localStorage.setItem("app.topView", view);
              } catch {
                // storage unavailable — per-session default only.
              }
            }}
          >
            {view === "bib" ? "Bibliography" : "Wiki"}
          </button>
        ))}
        <span className="ml-auto text-xs text-muted-foreground">autonomics workspace</span>
      </nav>
      <div className="min-h-0 flex-1">{topView === "wiki" ? <WikiPanel /> : bibPane}</div>
    </div>
  );
}
