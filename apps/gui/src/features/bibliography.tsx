import { useCallback, useEffect, useMemo, useState } from "react";
import { gatewayRequest, pickUploadFile, saveTextFile, uploadBibFulltext } from "@/gateway";
import type { BibArticle, BibCollection, BibDetail } from "@/types";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

function articleAuthors(article: BibArticle) {
  return (article.authors ?? [])
    .map((author) => author.name || [author.fore_name, author.last_name].filter(Boolean).join(" "))
    .filter(Boolean)
    .join(", ");
}

function articleIdentifier(article: BibArticle, kind: string) {
  return article.identifiers?.find((item) => item.kind.toLowerCase() === kind)?.value ?? "—";
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
  }, [query]);

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

  const detailText = () => {
    if (!selected) return "No article selected";
    if (detailLoading) return "Loading…";
    if (!detail) return "";
    const fulltext = detail.fulltext;
    if (detail.article.id !== selected.id) return "";
    return [
      `Title: ${selected.title}`,
      `Authors: ${articleAuthors(selected) || "—"}`,
      `Year: ${selected.year ?? "—"}`,
      `Journal: ${selected.journal ?? "—"}`,
      `DOI: ${articleIdentifier(selected, "doi")}`,
      `Source: ${selected.source ?? "—"}`,
      "",
      "Abstract",
      selected.abstract_text || "Not available",
      "",
      "Full text",
      fulltext?.text_content || `Extraction: ${fulltext?.extract_status ?? "not uploaded"}`,
      "",
      "Notes",
      detail.annotations.map((note) => `[${note.kind ?? "note"}] ${note.content}`).join("\n") || "No notes yet",
    ].join("\n");
  };

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
          <Button type="submit" disabled={loading}>Search</Button>
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

      <Card className="flex min-h-0 flex-col overflow-auto p-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="min-w-0">
            <h2 className="truncate text-base font-medium">{selected?.title ?? "Bibliography"}</h2>
            <p className="truncate text-xs text-muted-foreground">{selected?.id ?? "Select an article"}</p>
          </div>
          <div className="flex gap-2">
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
        <Separator className="my-3" />
        {error && (
          <div className="mb-3 rounded-md border border-destructive/40 bg-destructive/10 p-2 text-sm text-destructive">
            {error}
          </div>
        )}
        <Tabs defaultValue="metadata" className="flex min-h-0 flex-1 flex-col">
          <TabsList>
            <TabsTrigger value="metadata">Metadata</TabsTrigger>
            <TabsTrigger value="fulltext">Full text</TabsTrigger>
            <TabsTrigger value="notes">Notes</TabsTrigger>
          </TabsList>
          <TabsContent value="metadata" className="min-h-0 flex-1 overflow-auto">
            <pre className="whitespace-pre-wrap font-sans text-sm">{detailText()}</pre>
          </TabsContent>
          <TabsContent value="fulltext" className="min-h-0 flex-1 overflow-auto">
            <pre className="whitespace-pre-wrap font-sans text-sm">{detailText()}</pre>
          </TabsContent>
          <TabsContent value="notes" className="min-h-0 flex-1 overflow-auto">
            <pre className="whitespace-pre-wrap font-sans text-sm">{detailText()}</pre>
          </TabsContent>
        </Tabs>
      </Card>
    </div>
  );
}
