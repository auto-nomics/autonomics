import { useEffect, useState } from "react";
import { RotateCcw, Trash2 } from "lucide-react";
import { gatewayRequest } from "@/gateway";
import type { BibArticle } from "@/types";
import { Button } from "@/components/ui/button";
import { SearchDialog } from "./search-dialog";

export function ArticleTrash({ onClose, onChanged }: { onClose: () => void; onChanged: () => void }) {
  const [articles, setArticles] = useState<BibArticle[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const load = async (append = false) => {
    setLoading(true);
    try { const result = await gatewayRequest("GET", `/bib/articles/trash?limit=50&offset=${append ? articles.length : 0}`); const rows = (result.articles ?? []).map((entry: { article: BibArticle }) => entry.article); setArticles(current => append ? [...current, ...rows] : rows); setTotal(result.total ?? rows.length); }
    catch (failure) { setError(String(failure)); }
    finally { setLoading(false); }
  };
  useEffect(() => { void load(); }, []);
  const change = async (article: BibArticle, permanent: boolean) => {
    if (permanent && !window.confirm(`Permanently delete “${article.title}” and its attachments and notes? This cannot be undone.`)) return;
    setBusy(true); setError("");
    try {
      await gatewayRequest(permanent ? "DELETE" : "POST", `/bib/articles/${encodeURIComponent(article.id)}/${permanent ? "permanent" : "restore"}`);
      setArticles(current => current.filter(item => item.id !== article.id)); setTotal(current => Math.max(0, current - 1)); onChanged();
    } catch (failure) { setError(String(failure)); }
    finally { setBusy(false); }
  };
  return <SearchDialog title="Article trash" onClose={() => { if (!busy) onClose(); }}>
    <p className="mb-3 text-sm text-muted-foreground">Trashed articles retain their attachments, notes, and collection membership until permanently deleted.</p>
    {error && <div role="alert" className="mb-3 text-sm text-destructive">{error}<Button size="xs" variant="ghost" disabled={busy || loading} onClick={() => { setError(""); void load(); }}>Retry</Button></div>}
    {loading && <p role="status">Loading…</p>}
    {articles.length ? <ul className="space-y-2">{articles.map(article => <li key={article.id} className="flex items-center gap-2 rounded border p-2">
      <span className="min-w-0 flex-1 break-words">{article.title}</span>
      <Button size="icon-sm" variant="ghost" aria-label={`Restore ${article.title}`} title="Restore article" disabled={busy || loading} onClick={() => void change(article, false)}><RotateCcw /></Button>
      <Button size="icon-sm" variant="destructive" aria-label={`Permanently delete ${article.title}`} title="Delete permanently" disabled={busy || loading} onClick={() => void change(article, true)}><Trash2 /></Button>
    </li>)}</ul> : !loading && <p className="text-sm text-muted-foreground">No trashed articles</p>}
    {articles.length < total && <Button className="mt-3" variant="outline" disabled={busy || loading} onClick={() => void load(true)}>Load more ({articles.length} / {total})</Button>}
  </SearchDialog>;
}
