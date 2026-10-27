import { useEffect, useState } from "react";
import { gatewayRequest } from "@/gateway";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SearchDialog } from "./search-dialog";
import { builderIsLossless, formatExpression, searchFieldLabels, type SearchExpr, type SearchField } from "./bib-search-model";

const blank = (): SearchExpr => ({ op: "term", value: "", field: "any" });
function canonical(expr: SearchExpr): unknown {
  if (expr.op === "and" || expr.op === "or") return expr.clauses.length === 1 ? canonical(expr.clauses[0]) : { op: expr.op, clauses: expr.clauses.map(canonical) };
  if (expr.op === "not") return { op: "not", clause: canonical(expr.clause) };
  if (expr.op === "range") return { op: "range", field: expr.field, gte: expr.gte || null, lte: expr.lte || null };
  if (!("value" in expr)) throw new Error("Unsupported expression");
  return { op: /^[\p{L}\p{N}]+$/u.test(expr.value) ? "word" : expr.op, value: expr.value, field: !expr.field || expr.field === "any" ? null : expr.field };
}
function Editor({ value, onChange, depth = 0 }: { value: SearchExpr; onChange: (expr: SearchExpr) => void; depth?: number }) {
  const group = value.op === "and" || value.op === "or";
  const changeOp = (op: SearchExpr["op"]) => {
    if (op === "and" || op === "or") onChange({ op, clauses: group ? value.clauses : [value] });
    else if (op === "not") onChange({ op, clause: value });
    else if (op === "range") onChange({ op, field: "year", gte: "", lte: "" });
    else onChange({ op, value: "value" in value ? value.value : "", field: "field" in value ? value.field : "any" });
  };
  return <fieldset className="min-w-0 space-y-2 rounded border p-2">
    <legend className="sr-only">{group ? "Condition group" : "Search condition"}</legend>
    <div className="flex flex-wrap items-center gap-2">
      <select aria-label="Condition operator" className="rounded border bg-background p-1 text-xs" value={value.op} onChange={event => changeOp(event.target.value as SearchExpr["op"])}>
        <option value="and">All (AND)</option><option value="or">Any (OR)</option><option value="not">Exclude (NOT)</option><option value="term">Word</option><option value="phrase">Exact phrase</option><option value="range">Year range</option>
      </select>
      {(value.op === "term" || value.op === "phrase") && <>
        <select aria-label="Condition field" className="rounded border bg-background p-1 text-xs" value={value.field ?? "any"} onChange={event => onChange({ ...value, field: event.target.value as SearchField })}>{Object.entries(searchFieldLabels).map(([field, label]) => <option key={field} value={field}>{label}</option>)}</select>
        <Input aria-label="Condition value" className="h-7 min-w-28 flex-1 text-xs" value={value.value} onChange={event => onChange({ ...value, value: event.target.value })} placeholder={value.op === "phrase" ? "Words in this exact order" : "One word or identifier"} />
      </>}
      {value.op === "range" && <>
        <Input aria-label="Year range from" className="h-7 w-28" type="number" min="0" max="9999" value={value.gte ?? ""} onChange={event => onChange({ ...value, gte: event.target.value })} />
        <span>to</span><Input aria-label="Year range to" className="h-7 w-28" type="number" min="0" max="9999" value={value.lte ?? ""} onChange={event => onChange({ ...value, lte: event.target.value })} />
      </>}
    </div>
    {group && <div className="space-y-2 pl-2">
      {value.clauses.map((clause, index) => <div key={index} className="flex items-start gap-1"><div className="min-w-0 flex-1"><Editor value={clause} depth={depth + 1} onChange={expr => onChange({ ...value, clauses: value.clauses.map((old, i) => i === index ? expr : old) })} /></div><Button size="xs" variant="ghost" aria-label={`Remove condition ${index + 1}`} disabled={value.clauses.length <= 1} onClick={() => onChange({ ...value, clauses: value.clauses.filter((_, i) => i !== index) })}>×</Button></div>)}
      <Button size="xs" variant="secondary" onClick={() => onChange({ ...value, clauses: [...value.clauses, blank()] })}>Add condition</Button>
      <Button size="xs" variant="ghost" disabled={depth >= 12} onClick={() => onChange({ ...value, clauses: [...value.clauses, { op: "or", clauses: [blank()] }] })}>Add group</Button>
    </div>}
    {value.op === "not" && <div className="pl-2"><Editor value={value.clause} depth={depth + 1} onChange={clause => onChange({ ...value, clause })} /></div>}
  </fieldset>;
}

export function AdvancedSearchBuilder({ query, expression, onApply, onClose }: { query: string; expression?: SearchExpr | null; onApply: (expr: SearchExpr, text: string) => void; onClose: () => void }) {
  const [value, setValue] = useState<SearchExpr | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    const abort = new AbortController();
    const load = async () => {
      try {
        const parsed = expression ?? (await gatewayRequest("POST", "/bib/search/parse", { query }, { signal: abort.signal })).expression ?? { op: "and", clauses: [blank()] };
        if (!builderIsLossless(parsed)) throw new Error("This query cannot be edited without changing its meaning. Keep editing it in the search box.");
        setValue(parsed);
      } catch (failure) { if (!abort.signal.aborted) setError(String(failure)); }
    };
    void load();
    return () => abort.abort();
  }, [expression, query]);
  const apply = async () => {
    if (!value) return;
    setBusy(true); setError("");
    try {
      if (!builderIsLossless(value)) throw new Error("Use Exact phrase for text with spaces, or split it into separate conditions.");
      const text = formatExpression(value);
      const parsed = (await gatewayRequest("POST", "/bib/search/parse", { query: text })).expression;
      if (!parsed || JSON.stringify(canonical(value)) !== JSON.stringify(canonical(parsed))) throw new Error("This condition cannot be represented faithfully in the query language. Edit the query in the search box instead.");
      onApply(value, text);
    } catch (failure) { setError(String(failure)); } finally { setBusy(false); }
  };
  return <SearchDialog title="Advanced search" onClose={onClose}>
    <p className="mb-3 text-xs text-muted-foreground">Group conditions with AND, OR and NOT. Explicit fields override the quick search field selection.</p>
    {value && <Editor value={value} onChange={setValue} />}
    {value && <pre aria-label="Query preview" className="mt-3 whitespace-pre-wrap break-words rounded bg-muted p-2 text-xs">{formatExpression(value)}</pre>}
    {error && <p role="alert" className="mt-2 text-xs text-destructive">{error}</p>}
    {!value && !error && <p role="status">Loading conditions…</p>}
    <div className="mt-3 flex justify-end gap-2"><Button size="sm" variant="ghost" onClick={onClose}>Cancel</Button><Button size="sm" disabled={!value || busy} onClick={() => void apply()}>Apply search</Button></div>
  </SearchDialog>;
}
