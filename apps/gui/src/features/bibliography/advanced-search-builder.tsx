import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
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
  const { t } = useTranslation("bibliography");
  const group = value.op === "and" || value.op === "or";
  const changeOp = (op: SearchExpr["op"]) => {
    if (op === "and" || op === "or") onChange({ op, clauses: group ? value.clauses : [value] });
    else if (op === "not") onChange({ op, clause: value });
    else if (op === "range") onChange({ op, field: "year", gte: "", lte: "" });
    else onChange({ op, value: "value" in value ? value.value : "", field: "field" in value ? value.field : "any" });
  };
  return <fieldset className="min-w-0 space-y-2 rounded border p-2">
    <legend className="sr-only">{group ? t("advancedSearch.conditionGroup") : t("advancedSearch.searchCondition")}</legend>
    <div className="flex flex-wrap items-center gap-2">
      <select aria-label={t("advancedSearch.operatorAria")} className="rounded border bg-background p-1 text-xs" value={value.op} onChange={event => changeOp(event.target.value as SearchExpr["op"])}>
        <option value="and">{t("advancedSearch.opAll")}</option><option value="or">{t("advancedSearch.opAny")}</option><option value="not">{t("advancedSearch.opNot")}</option><option value="term">{t("advancedSearch.opWord")}</option><option value="phrase">{t("advancedSearch.opPhrase")}</option><option value="range">{t("advancedSearch.opRange")}</option>
      </select>
      {(value.op === "term" || value.op === "phrase") && <>
        <select aria-label={t("advancedSearch.fieldAria")} className="rounded border bg-background p-1 text-xs" value={value.field ?? "any"} onChange={event => onChange({ ...value, field: event.target.value as SearchField })}>{Object.keys(searchFieldLabels).map(field => <option key={field} value={field}>{t(`advancedSearch.fields.${field}`)}</option>)}</select>
        <Input aria-label={t("advancedSearch.valueAria")} className="h-7 min-w-28 flex-1 text-xs" value={value.value} onChange={event => onChange({ ...value, value: event.target.value })} placeholder={value.op === "phrase" ? t("advancedSearch.phrasePlaceholder") : t("advancedSearch.termPlaceholder")} />
      </>}
      {value.op === "range" && <>
        <Input aria-label={t("advancedSearch.rangeFromAria")} className="h-7 w-28" type="number" min="0" max="9999" value={value.gte ?? ""} onChange={event => onChange({ ...value, gte: event.target.value })} />
        <span>{t("advancedSearch.rangeTo")}</span><Input aria-label={t("advancedSearch.rangeToAria")} className="h-7 w-28" type="number" min="0" max="9999" value={value.lte ?? ""} onChange={event => onChange({ ...value, lte: event.target.value })} />
      </>}
    </div>
    {group && <div className="space-y-2 pl-2">
      {value.clauses.map((clause, index) => <div key={index} className="flex items-start gap-1"><div className="min-w-0 flex-1"><Editor value={clause} depth={depth + 1} onChange={expr => onChange({ ...value, clauses: value.clauses.map((old, i) => i === index ? expr : old) })} /></div><Button size="xs" variant="ghost" aria-label={t("advancedSearch.removeConditionAria", { index: index + 1 })} disabled={value.clauses.length <= 1} onClick={() => onChange({ ...value, clauses: value.clauses.filter((_, i) => i !== index) })}>×</Button></div>)}
      <Button size="xs" variant="secondary" onClick={() => onChange({ ...value, clauses: [...value.clauses, blank()] })}>{t("advancedSearch.addCondition")}</Button>
      <Button size="xs" variant="ghost" disabled={depth >= 12} onClick={() => onChange({ ...value, clauses: [...value.clauses, { op: "or", clauses: [blank()] }] })}>{t("advancedSearch.addGroup")}</Button>
    </div>}
    {value.op === "not" && <div className="pl-2"><Editor value={value.clause} depth={depth + 1} onChange={clause => onChange({ ...value, clause })} /></div>}
  </fieldset>;
}

export function AdvancedSearchBuilder({ query, expression, onApply, onClose }: { query: string; expression?: SearchExpr | null; onApply: (expr: SearchExpr, text: string) => void; onClose: () => void }) {
  const { t } = useTranslation("bibliography");
  const [value, setValue] = useState<SearchExpr | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    const abort = new AbortController();
    const load = async () => {
      try {
        const parsed = expression ?? (await gatewayRequest("POST", "/bib/search/parse", { query }, { signal: abort.signal })).expression ?? { op: "and", clauses: [blank()] };
        if (!builderIsLossless(parsed)) throw new Error(t("advancedSearch.losslessError"));
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
      if (!builderIsLossless(value)) throw new Error(t("advancedSearch.phraseRequiredError"));
      const text = formatExpression(value);
      const parsed = (await gatewayRequest("POST", "/bib/search/parse", { query: text })).expression;
      if (!parsed || JSON.stringify(canonical(value)) !== JSON.stringify(canonical(parsed))) throw new Error(t("advancedSearch.representableError"));
      onApply(value, text);
    } catch (failure) { setError(String(failure)); } finally { setBusy(false); }
  };
  return <SearchDialog title={t("advancedSearch.title")} onClose={onClose}>
    <p className="mb-3 text-xs text-muted-foreground">{t("advancedSearch.hint")}</p>
    {value && <Editor value={value} onChange={setValue} />}
    {value && <pre aria-label={t("advancedSearch.previewAria")} className="mt-3 whitespace-pre-wrap break-words rounded bg-muted p-2 text-xs">{formatExpression(value)}</pre>}
    {error && <p role="alert" className="mt-2 text-xs text-destructive">{error}</p>}
    {!value && !error && <p role="status">{t("advancedSearch.loading")}</p>}
    <div className="mt-3 flex justify-end gap-2"><Button size="sm" variant="ghost" onClick={onClose}>{t("advancedSearch.cancel")}</Button><Button size="sm" disabled={!value || busy} onClick={() => void apply()}>{t("advancedSearch.apply")}</Button></div>
  </SearchDialog>;
}
