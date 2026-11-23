import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SearchDialog } from "./search-dialog";
import { formatExpression, type SavedSearch, type SearchRequest } from "./bib-search-model";

type Props = { disabled?: boolean; scopeLabel: string; request: SearchRequest; activeId: string | null; onSelect: (rule: SavedSearch) => void; onSaved: (rule: SavedSearch) => void; onDeleted: (id: string) => void };
export function SavedSearches({ disabled, scopeLabel, request, activeId, onSelect, onSaved, onDeleted }: Props) {
  const { t } = useTranslation("bibliography");
  const [rules, setRules] = useState<SavedSearch[]>([]);
  const [dialog, setDialog] = useState<{ kind: "create" | "update" | "delete"; rule?: SavedSearch } | null>(null);
  const [name, setName] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const load = useCallback(async () => {
    try { setRules((await gatewayRequest("GET", "/bib/saved-searches")).saved_searches ?? []); }
    catch (failure) { setError(String(failure)); }
  }, []);
  useEffect(() => { void load(); const focus = () => void load(); window.addEventListener("focus", focus); return () => window.removeEventListener("focus", focus); }, [load]);
  const active = rules.find(rule => rule.id === activeId);
  const submit = async () => {
    if (!dialog) return;
    setBusy(true); setError("");
    try {
      const rule = dialog.rule;
      if (dialog.kind === "delete" && rule) {
        await gatewayRequest("DELETE", `/bib/saved-searches/${encodeURIComponent(rule.id)}?expected_version=${rule.version}`);
        onDeleted(rule.id);
      } else {
        const body = { name, request: { ...request, cursor: null }, expected_version: rule?.version ?? null };
        const response = await gatewayRequest(rule ? "PUT" : "POST", rule ? `/bib/saved-searches/${encodeURIComponent(rule.id)}` : "/bib/saved-searches", body);
        onSaved(response.saved_search);
      }
      await load(); setDialog(null);
    } catch (failure) { setError(String(failure)); await load(); } finally { setBusy(false); }
  };
  const open = (kind: "create" | "update" | "delete", rule?: SavedSearch) => { setError(""); setName(rule?.name ?? ""); setDialog({ kind, rule }); };
  return <section aria-label={t("savedSearches.sectionAria")} className="mt-3 border-t px-1 pt-2 text-xs">
    <div className="mb-1 flex items-center justify-between"><span className="font-medium text-muted-foreground">{t("savedSearches.heading")}</span><Button size="icon-xs" variant="ghost" aria-label={t("savedSearches.saveCurrent")} title={t("savedSearches.saveCurrent")} disabled={disabled} onClick={() => open("create")}>+</Button></div>
    {!rules.length && <p className="px-1 py-2 text-muted-foreground">{t("savedSearches.empty")}</p>}
    {rules.map(rule => <div key={rule.id} className={`flex items-center gap-1 rounded ${rule.id === activeId ? "bg-accent" : ""}`}>
      <button className="min-w-0 flex-1 truncate p-1.5 text-left hover:underline" aria-pressed={rule.id === activeId} disabled={disabled} title={rule.name} onClick={() => onSelect(rule)}>⌕ {rule.name}</button>
      <Button size="icon-xs" variant="ghost" aria-label={t("savedSearches.deleteAria", { name: rule.name })} onClick={() => open("delete", rule)}>×</Button>
    </div>)}
    {active && <Button size="xs" variant="ghost" disabled={disabled} onClick={() => open("update", active)}>{t("savedSearches.update")}</Button>}
    {error && !dialog && <p role="alert" className="p-1 text-destructive">{error}<button className="ml-1 underline" onClick={() => void load()}>{t("savedSearches.refresh")}</button></p>}
    {dialog && <SearchDialog title={dialog.kind === "delete" ? t("savedSearches.deleteTitle") : dialog.kind === "update" ? t("savedSearches.updateTitle") : t("savedSearches.saveTitle")} onClose={() => setDialog(null)}>
      {dialog.kind === "delete" ? <p>{t("savedSearches.deleteConfirm", { name: dialog.rule?.name ?? "" })}</p> : <>
        <label className="block text-xs">{t("savedSearches.nameLabel")}<Input aria-label={t("savedSearches.nameLabel")} autoFocus maxLength={120} value={name} onChange={event => setName(event.target.value)} /></label>
        <p className="mt-2 text-xs text-muted-foreground">{t("savedSearches.ruleHint")}</p>
        {request.mode === "hybrid" && <p className="mt-2 whitespace-pre-wrap break-words rounded bg-muted p-2 text-xs"><strong>{t("savedSearches.topic")}</strong> {request.semantic_query}</p>}
        <pre className="mt-2 whitespace-pre-wrap break-words rounded bg-muted p-2 text-xs">{request.mode === "hybrid" ? t("savedSearches.requiredConditions") : ""}{request.expression ? formatExpression(request.expression) : request.query || t("savedSearches.allArticles")}</pre>
        {request.mode === "hybrid" && <p className="mt-2 text-xs text-muted-foreground">{t("savedSearches.hybridHint")}</p>}
        <p className="mt-2 text-xs">{scopeLabel} · {t("savedSearches.sorting", { sort: request.sort ?? "relevance" })} · {t("savedSearches.filterGroups", { count: Object.values(request.filters ?? {}).filter(value => Array.isArray(value) ? value.length : value != null).length })}</p>
      </>}
      {error && <p role="alert" className="mt-2 text-xs text-destructive">{error}</p>}
      <div className="mt-3 flex justify-end gap-2"><Button size="sm" variant="ghost" onClick={() => setDialog(null)}>{t("savedSearches.cancel")}</Button><Button size="sm" disabled={busy || (dialog.kind !== "delete" && !name.trim())} onClick={() => void submit()}>{dialog.kind === "delete" ? t("savedSearches.deleteRule") : t("savedSearches.saveRule")}</Button></div>
    </SearchDialog>}
  </section>;
}
