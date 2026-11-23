import { useEffect, useState } from "react";
import { RotateCcw, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { gatewayRequest } from "@/gateway";
import type { BibArticle } from "@/types";
import { Button } from "@/components/ui/button";
import { SearchDialog } from "./search-dialog";

export function ArticleTrash({ onClose, onChanged }: { onClose: () => void; onChanged: () => void }) {
  const { t } = useTranslation("bibliography");
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
    if (permanent && !window.confirm(t("articleTrash.permanentConfirm", { title: article.title }))) return;
    setBusy(true); setError("");
    try {
      await gatewayRequest(permanent ? "DELETE" : "POST", `/bib/articles/${encodeURIComponent(article.id)}/${permanent ? "permanent" : "restore"}`);
      setArticles(current => current.filter(item => item.id !== article.id)); setTotal(current => Math.max(0, current - 1)); onChanged();
    } catch (failure) { setError(String(failure)); }
    finally { setBusy(false); }
  };
  return <SearchDialog title={t("articleTrash.title")} onClose={() => { if (!busy) onClose(); }}>
    <p className="mb-3 text-sm text-muted-foreground">{t("articleTrash.description")}</p>
    {error && <div role="alert" className="mb-3 text-sm text-destructive">{error}<Button size="xs" variant="ghost" disabled={busy || loading} onClick={() => { setError(""); void load(); }}>{t("articleTrash.retry")}</Button></div>}
    {loading && <p role="status">{t("articleTrash.loading")}</p>}
    {articles.length ? <ul className="space-y-2">{articles.map(article => <li key={article.id} className="flex items-center gap-2 rounded border p-2">
      <span className="min-w-0 flex-1 break-words">{article.title}</span>
      <Button size="icon-sm" variant="ghost" aria-label={t("articleTrash.restoreAria", { title: article.title })} title={t("articleTrash.restoreTitle")} disabled={busy || loading} onClick={() => void change(article, false)}><RotateCcw /></Button>
      <Button size="icon-sm" variant="destructive" aria-label={t("articleTrash.deleteAria", { title: article.title })} title={t("articleTrash.deleteTitle")} disabled={busy || loading} onClick={() => void change(article, true)}><Trash2 /></Button>
    </li>)}</ul> : !loading && <p className="text-sm text-muted-foreground">{t("articleTrash.empty")}</p>}
    {articles.length < total && <Button className="mt-3" variant="outline" disabled={busy || loading} onClick={() => void load(true)}>{t("articleTrash.loadMore", { loaded: articles.length, total })}</Button>}
  </SearchDialog>;
}
