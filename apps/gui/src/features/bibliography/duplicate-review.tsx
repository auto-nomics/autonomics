import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { SearchDialog } from "./search-dialog";
import type { BibArticle } from "@/types";
const mergeFields = ["title","authors","year","journal","volume","issue","pages","abstract_text","extra_fields"] as const;
type Group = { reason: string; articles: BibArticle[]; revisions: Record<string,string> };
export function DuplicateReview({ onClose, onMerged }: { onClose: () => void; onMerged: () => void }) {
  const { t } = useTranslation("bibliography");
  const [groups, setGroups] = useState<Group[]>([]);
  const [targets, setTargets] = useState<Record<string,string>>({});
  const [fieldSources, setFieldSources] = useState<Record<string, Record<string,string>>>({});
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState("");
  const [truncated, setTruncated] = useState(false);
  const load = async () => {
    setBusy(true); setError("");
    try { const response = await gatewayRequest("GET", "/bib/duplicates"); setGroups(response.groups); setTruncated(response.truncated); setTargets({}); setFieldSources({}); }
    catch (failure) { setError(String(failure)); } finally { setBusy(false); }
  };
  useEffect(() => { void load(); }, []);
  const merge = async (group: Group) => {
    const target = targets[group.reason] || group.articles[0].id;
    if (!window.confirm(t("duplicateReview.mergeConfirm"))) return;
    const metadata = { ...group.articles.find(article => article.id === target)! };
    for (const field of mergeFields) {
      const source = group.articles.find(article => article.id === fieldSources[group.reason]?.[field]);
      if (source) Object.assign(metadata, { [field]: source[field] ?? (field === "extra_fields" ? {} : field === "authors" ? [] : null) });
    }
    setBusy(true); setError("");
    try {
      await gatewayRequest("POST", "/bib/duplicates/merge", { target_id: target, source_ids: group.articles.filter(article => article.id !== target).map(article => article.id), expected_revisions: group.revisions, metadata });
      onMerged(); await load();
    } catch (failure) { setError(String(failure)); setBusy(false); }
  };
  return <SearchDialog title={t("duplicateReview.title")} onClose={() => { if (!busy) onClose(); }} footer={<Button variant="outline" disabled={busy} onClick={() => void load()}>{t("duplicateReview.refresh")}</Button>}>
    <div className="space-y-3">
      <p className="text-xs text-muted-foreground">{t("duplicateReview.description")}</p>
      {error && <p role="alert" className="text-destructive">{error}</p>}
      {busy && <p role="status">{t("duplicateReview.working")}</p>}
      {truncated && <p>{t("duplicateReview.truncated")}</p>}
      {!busy && !groups.length && !error && <p>{t("duplicateReview.empty")}</p>}
      {groups.map(group => <fieldset key={group.reason} className="space-y-2 rounded border p-3" disabled={busy}>
        <legend className="max-w-full truncate text-xs">{group.reason}</legend>
        {group.articles.map(article => <label key={article.id} className="flex items-start gap-2">
          <input type="radio" name={group.reason} checked={(targets[group.reason] || group.articles[0].id) === article.id} onChange={() => setTargets(current => ({ ...current, [group.reason]: article.id }))} />
          <span><span className="block">{article.title}</span><span className="block text-xs text-muted-foreground">{article.year ?? t("duplicateReview.noYear")} · {article.journal || t("duplicateReview.noPublication")} · {article.id}</span>
            <details className="text-xs"><summary>{t("duplicateReview.compareMetadata")}</summary><pre className="whitespace-pre-wrap break-all">{JSON.stringify(article, null, 2)}</pre></details>
          </span>
        </label>)}
        <div className="grid grid-cols-1 gap-2 rounded bg-muted/40 p-2">
          {mergeFields.map(field => { const label = t(`duplicateReview.fields.${field}`); return <label key={field} className="grid gap-1 text-xs">
            <span>{label}</span>
            <select aria-label={t("duplicateReview.mergedFieldAria", { label })} className="min-w-0 rounded border bg-background p-1"
              value={fieldSources[group.reason]?.[field] || targets[group.reason] || group.articles[0].id}
              onChange={event => setFieldSources(current => ({ ...current, [group.reason]: { ...current[group.reason], [field]: event.target.value } }))}>
              {group.articles.map((article, index) => <option key={article.id} value={article.id}>
                {index + 1}: {typeof article[field] === "object" ? JSON.stringify(article[field]) : String(article[field] ?? t("duplicateReview.emptyValue"))}
              </option>)}
            </select>
          </label>; })}
        </div>
        <Button size="sm" disabled={busy} onClick={() => void merge(group)}>{t("duplicateReview.mergeButton")}</Button>
      </fieldset>)}
    </div>
  </SearchDialog>;
}
