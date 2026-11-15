import { useEffect, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { SearchDialog } from "./search-dialog";
import type { BibArticle } from "@/types";
const mergeFields = [["title","Title"],["authors","Authors"],["year","Year"],["journal","Publication"],["volume","Volume"],["issue","Issue"],["pages","Pages"],["abstract_text","Abstract"],["extra_fields","Type-specific fields"]] as const;
type Group = { reason: string; articles: BibArticle[]; revisions: Record<string,string> };
export function DuplicateReview({ onClose, onMerged }: { onClose: () => void; onMerged: () => void }) {
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
    if (!window.confirm("Keep the reviewed field values and combine tags, identifiers, attachments, notes and collection memberships? Other records will be permanently removed. This merge cannot be undone.")) return;
    const metadata = { ...group.articles.find(article => article.id === target)! };
    for (const [field] of mergeFields) {
      const source = group.articles.find(article => article.id === fieldSources[group.reason]?.[field]);
      if (source) Object.assign(metadata, { [field]: source[field] ?? (field === "extra_fields" ? {} : field === "authors" ? [] : null) });
    }
    setBusy(true); setError("");
    try {
      await gatewayRequest("POST", "/bib/duplicates/merge", { target_id: target, source_ids: group.articles.filter(article => article.id !== target).map(article => article.id), expected_revisions: group.revisions, metadata });
      onMerged(); await load();
    } catch (failure) { setError(String(failure)); setBusy(false); }
  };
  return <SearchDialog title="Review duplicate records" onClose={() => { if (!busy) onClose(); }} footer={<Button variant="outline" disabled={busy} onClick={() => void load()}>Refresh candidates</Button>}>
    <div className="space-y-3">
      <p className="text-xs text-muted-foreground">Candidates match an identifier or normalized title. Review publication versions before merging. Choose a primary record, then select individual field values. Conflicting identifiers require manual correction.</p>
      {error && <p role="alert" className="text-destructive">{error}</p>}
      {busy && <p role="status">Working…</p>}
      {truncated && <p>Showing the first 200 groups. Merge reviewed groups, then refresh.</p>}
      {!busy && !groups.length && !error && <p>No duplicate candidates found.</p>}
      {groups.map(group => <fieldset key={group.reason} className="space-y-2 rounded border p-3" disabled={busy}>
        <legend className="max-w-full truncate text-xs">{group.reason}</legend>
        {group.articles.map(article => <label key={article.id} className="flex items-start gap-2">
          <input type="radio" name={group.reason} checked={(targets[group.reason] || group.articles[0].id) === article.id} onChange={() => setTargets(current => ({ ...current, [group.reason]: article.id }))} />
          <span><span className="block">{article.title}</span><span className="block text-xs text-muted-foreground">{article.year ?? "No year"} · {article.journal || "No publication"} · {article.id}</span>
            <details className="text-xs"><summary>Compare metadata</summary><pre className="whitespace-pre-wrap break-all">{JSON.stringify(article, null, 2)}</pre></details>
          </span>
        </label>)}
        <div className="grid grid-cols-1 gap-2 rounded bg-muted/40 p-2">
          {mergeFields.map(([field, label]) => <label key={field} className="grid gap-1 text-xs">
            <span>{label}</span>
            <select aria-label={`Merged ${label}`} className="min-w-0 rounded border bg-background p-1"
              value={fieldSources[group.reason]?.[field] || targets[group.reason] || group.articles[0].id}
              onChange={event => setFieldSources(current => ({ ...current, [group.reason]: { ...current[group.reason], [field]: event.target.value } }))}>
              {group.articles.map((article, index) => <option key={article.id} value={article.id}>
                {index + 1}: {typeof article[field] === "object" ? JSON.stringify(article[field]) : String(article[field] ?? "(empty)")}
              </option>)}
            </select>
          </label>)}
        </div>
        <Button size="sm" disabled={busy} onClick={() => void merge(group)}>Merge into selected record</Button>
      </fieldset>)}
    </div>
  </SearchDialog>;
}
