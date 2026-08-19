import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";

import { ArticleDetailPanel } from "@/components/bibliography/article-detail";
import { CollectionTree, type CollectionView } from "@/components/bibliography/collection-tree";
import { RecordList } from "@/components/bibliography/record-list";
import {
  api,
  articleIdentifier,
  type Article,
  type ArticleDetail,
  type BibHealth,
  type Collection,
} from "@/lib/bib-api";


function matchesQuery(article: Article, query: string): boolean {
  if (!query.trim()) return true;
  const haystack = [
    article.title,
    article.abstract_text ?? "",
    article.journal ?? "",
    ...article.authors.map((author) => `${author.last_name} ${author.fore_name ?? ""}`),
  ]
    .join(" ")
    .toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((term) => haystack.includes(term));
}

export function App() {
  const [health, setHealth] = useState<BibHealth | null>(null);
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
    const data = await api<{ collections: Collection[] }>("/api/v1/bib/collections");
    setCollections(data.collections);
    return data.collections;
  }, []);

  const selectArticle = useCallback(async (id: string) => {
    const data = await api<ArticleDetail>(`/api/v1/bib/articles/${encodeURIComponent(id)}`);
    setSelected(data);
  }, []);

  const loadRecords = useCallback(
    async (view: CollectionView, searchTerm: string) => {
      setRecordsLoading(true);
      try {
        let nextRecords: Article[] = [];

        if (view === "all") {
          const data = await api<{ articles: Article[] }>(
            `/api/v1/bib/articles?limit=500&query=${encodeURIComponent(searchTerm.trim())}`,
          );
          nextRecords = data.articles;
        } else if (view === "unfiled") {
          const data = await api<{ articles: Article[] }>(
            `/api/v1/bib/articles/unfiled?limit=500`,
          );
          nextRecords = data.articles.filter((article) => matchesQuery(article, searchTerm));
        } else {
          const collectionId = view.slice("col:".length);
          const collectionData = await api<{ articles: Article[] }>(
            `/api/v1/bib/collections/${encodeURIComponent(collectionId)}/articles`,
          );
          const searchHits = searchTerm.trim()
            ? await api<{ articles: Article[] }>(
                `/api/v1/bib/articles?limit=500&query=${encodeURIComponent(searchTerm.trim())}`,
              )
            : { articles: collectionData.articles };
          const matchingIds = new Set(searchHits.articles.map((article) => article.id));
          nextRecords = collectionData.articles.filter((article) => matchingIds.has(article.id));
        }

        setRecords(nextRecords);
      } finally {
        setRecordsLoading(false);
      }
    },
    [],
  );

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
          body: JSON.stringify({ id_type: idType, id: id.trim(), fetch_fulltext: true }),
        },
      );

      if (activeView.startsWith("col:")) {
        const collectionId = activeView.slice("col:".length);
        await api(`/api/v1/bib/collections/${encodeURIComponent(collectionId)}/articles`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ article_id: data.article.id }),
        });
      }

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
    const form = new FormData();
    form.append("file", file);
    const response = await fetch(
      `/api/v1/bib/articles/${encodeURIComponent(selected.article.id)}/fulltext`,
      { method: "POST", body: form },
    );
    const data = await response.json().catch(() => null);
    if (!response.ok) throw new Error(data?.error ?? response.statusText);
    await selectArticle(selected.article.id);
    toast.success("Full text uploaded");
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

  async function createCollection(name: string) {
    const data = await api<{ collection: Collection }>("/api/v1/bib/collections", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name }),
    });
    await loadCollections();
    setActiveView(`col:${data.collection.id}`);
    setSelected(null);
    await loadRecords(`col:${data.collection.id}`, "");
    toast.success("Collection created");
  }

  async function deleteCollection(id: string) {
    await api(`/api/v1/bib/collections/${encodeURIComponent(id)}`, { method: "DELETE" });
    const nextView: CollectionView = activeView === `col:${id}` ? "all" : activeView;
    await loadCollections();
    setActiveView(nextView);
    await loadRecords(nextView, query);
    toast.success("Collection deleted");
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

  return (
    <div className="flex h-screen min-h-0 flex-col overflow-y-auto bg-background lg:overflow-hidden">

      <main className="grid w-full min-h-0 flex-1 gap-4 px-4 py-4 lg:grid-rows-[minmax(0,1fr)] lg:items-stretch lg:grid-cols-[260px_minmax(320px,2fr)_minmax(420px,3fr)] lg:px-6 xl:grid-cols-[280px_minmax(360px,2fr)_minmax(480px,3fr)]">
        <aside className="flex min-w-0 flex-col lg:min-h-0 lg:h-full">
          <CollectionTree
            collections={collections}
            activeView={activeView}
            allCount={health?.article_count ?? 0}
            onSelect={(view) => void changeView(view)}
            onCreate={createCollection}
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
}
