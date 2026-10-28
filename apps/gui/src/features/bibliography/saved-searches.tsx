import { useCallback, useEffect, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SearchDialog } from "./search-dialog";
import { formatExpression, type SavedSearch, type SearchRequest } from "./bib-search-model";

type Props = { disabled?: boolean; scopeLabel: string; request: SearchRequest; activeId: string | null; onSelect: (rule: SavedSearch) => void; onSaved: (rule: SavedSearch) => void; onDeleted: (id: string) => void };
export function SavedSearches({ disabled, scopeLabel, request, activeId, onSelect, onSaved, onDeleted }: Props) {
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
  return <section aria-label="Saved searches" className="mt-3 border-t px-1 pt-2 text-xs">
    <div className="mb-1 flex items-center justify-between"><span className="font-medium text-muted-foreground">Saved searches</span><Button size="icon-xs" variant="ghost" aria-label="Save current search" title="Save current search" disabled={disabled} onClick={() => open("create")}>+</Button></div>
    {!rules.length && <p className="px-1 py-2 text-muted-foreground">Save a query to follow matching articles.</p>}
    {rules.map(rule => <div key={rule.id} className={`flex items-center gap-1 rounded ${rule.id === activeId ? "bg-accent" : ""}`}>
      <button className="min-w-0 flex-1 truncate p-1.5 text-left hover:underline" aria-pressed={rule.id === activeId} disabled={disabled} title={rule.name} onClick={() => onSelect(rule)}>⌕ {rule.name}</button>
      <Button size="icon-xs" variant="ghost" aria-label={`Delete saved search ${rule.name}`} onClick={() => open("delete", rule)}>×</Button>
    </div>)}
    {active && <Button size="xs" variant="ghost" disabled={disabled} onClick={() => open("update", active)}>Update saved search</Button>}
    {error && !dialog && <p role="alert" className="p-1 text-destructive">{error}<button className="ml-1 underline" onClick={() => void load()}>Refresh saved searches</button></p>}
    {dialog && <SearchDialog title={dialog.kind === "delete" ? "Delete saved search" : dialog.kind === "update" ? "Update saved search" : "Save search"} onClose={() => setDialog(null)}>
      {dialog.kind === "delete" ? <p>Delete “{dialog.rule?.name}”? The articles remain in your library.</p> : <>
        <label className="block text-xs">Search name<Input aria-label="Search name" autoFocus maxLength={120} value={name} onChange={event => setName(event.target.value)} /></label>
        <p className="mt-2 text-xs text-muted-foreground">This rule follows the current query, scope and filters. New matching articles appear when the search runs.</p>
        {request.mode === "hybrid" && <p className="mt-2 whitespace-pre-wrap break-words rounded bg-muted p-2 text-xs"><strong>Topic:</strong> {request.semantic_query}</p>}
        <pre className="mt-2 whitespace-pre-wrap break-words rounded bg-muted p-2 text-xs">{request.mode === "hybrid" ? "Required conditions: " : ""}{request.expression ? formatExpression(request.expression) : request.query || "All articles"}</pre>
        {request.mode === "hybrid" && <p className="mt-2 text-xs text-muted-foreground">This rule retains the topic and required conditions. Running it needs an enabled topic index; results are a retrieved candidate pool.</p>}
        <p className="mt-2 text-xs">{scopeLabel} · {request.sort ?? "relevance"} sorting · {Object.values(request.filters ?? {}).filter(value => Array.isArray(value) ? value.length : value != null).length} active filter groups</p>
      </>}
      {error && <p role="alert" className="mt-2 text-xs text-destructive">{error}</p>}
      <div className="mt-3 flex justify-end gap-2"><Button size="sm" variant="ghost" onClick={() => setDialog(null)}>Cancel</Button><Button size="sm" disabled={busy || (dialog.kind !== "delete" && !name.trim())} onClick={() => void submit()}>{dialog.kind === "delete" ? "Delete rule" : "Save rule"}</Button></div>
    </SearchDialog>}
  </section>;
}
