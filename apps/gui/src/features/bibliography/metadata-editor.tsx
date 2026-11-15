import { useRef, useState } from "react";
import { LoaderCircle, Plus, Save, Trash2 } from "lucide-react";
import { gatewayRequest } from "@/gateway";
import type { BibArticle } from "@/types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SearchDialog } from "./search-dialog";

const textFields = [
  ["title", "Title"], ["journal", "Publication"], ["volume", "Volume"],
  ["issue", "Issue"], ["pages", "Pages"], ["language", "Language"],
  ["issn", "ISSN"], ["essn", "Electronic ISSN"],
] as const;
const identifierKinds = ["doi", "pmid", "pmc", "arxiv", "embase", "biorxiv", "s2", "openalex", "other"];
const textareaClass = "min-h-24 w-full rounded-md border bg-background px-3 py-2 text-sm";

export function MetadataEditor({ article, onClose, onSaved }: {
  article: BibArticle;
  onClose: () => void;
  onSaved: (article: BibArticle) => void;
}) {
  // Keep the complete detail object: PUT replaces the record, including fields not shown here.
  const [draft, setDraft] = useState(() => ({ ...article }));
  const [year, setYear] = useState(String(article.year ?? ""));
  const [keywords, setKeywords] = useState((article.keywords ?? []).join("\n"));
  const [dirty, setDirty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const saving = useRef(false);
  const change = (patch: Partial<BibArticle>) => { setDraft(current => ({ ...current, ...patch })); setDirty(true); };
  const close = () => {
    if (!saving.current && (!dirty || window.confirm("Discard unsaved metadata changes?"))) onClose();
  };
  const save = async () => {
    if (saving.current) return;
    if (!draft.title.trim()) { setError("Title is required."); return; }
    if (year.trim() && (!/^\d{1,4}$/.test(year.trim()) || Number(year) < 1)) {
      setError("Year must be a whole number between 1 and 9999."); return;
    }
    if ((draft.authors ?? []).some(author => !author.last_name?.trim() && !author.name?.trim())) {
      setError("Each author needs a family or organization name."); return;
    }
    if ((draft.identifiers ?? []).some(identifier => !identifier.value.trim())) {
      setError("Each identifier needs a value. Remove unused rows."); return;
    }
    saving.current = true; setBusy(true); setError(null);
    try {
      const response = await gatewayRequest("PUT", `/bib/articles/${encodeURIComponent(article.id)}`, {
        ...draft, title: draft.title.trim(), year: year.trim() ? Number(year) : null,
        authors: (draft.authors ?? []).map(author => ({ ...author, last_name: (author.last_name || author.name || "").trim() })),
        identifiers: (draft.identifiers ?? []).map(identifier => ({ ...identifier, value: identifier.value.trim() })),
        keywords: [...new Set(keywords.split(/\r?\n/).map(value => value.trim()).filter(Boolean))],
      });
      onSaved(response.article);
    } catch (failure) {
      setError(`Unable to save metadata: ${String(failure)}`);
    } finally { saving.current = false; setBusy(false); }
  };
  return <SearchDialog title="Edit metadata" onClose={close} footer={
    <div className="flex items-center justify-end gap-2">
      <Button variant="ghost" disabled={busy} onClick={close}>Cancel</Button>
      <Button disabled={busy || !dirty} onClick={() => void save()}>
        {busy ? <LoaderCircle className="animate-spin" /> : <Save />} {busy ? "Saving…" : "Save changes"}
      </Button>
    </div>
  }>
    <form onSubmit={event => { event.preventDefault(); void save(); }}>
      {error && <div role="alert" className="mb-3 rounded border border-destructive/40 p-2 text-sm text-destructive">{error}</div>}
      <fieldset disabled={busy} className="space-y-4">
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          {textFields.map(([field, label]) => <label key={field} className={field === "title" ? "space-y-1 sm:col-span-2" : "space-y-1"}>
            <span className="text-xs font-medium">{label}{field === "title" ? " *" : ""}</span>
            <Input aria-label={label} value={draft[field] ?? ""} onChange={event => change({ [field]: (field === "title" ? event.target.value : event.target.value || null) })} />
          </label>)}
          <label className="space-y-1"><span className="text-xs font-medium">Year</span>
            <Input aria-label="Year" inputMode="numeric" value={year} onChange={event => { setYear(event.target.value); setDirty(true); }} />
          </label>
        </div>
        <fieldset className="space-y-2"><legend className="text-xs font-medium">Authors (in publication order)</legend>
          {(draft.authors ?? []).map((author, index) => <div key={index} className="flex items-center gap-2">
            <Input aria-label={`Author ${index + 1} family name`} placeholder="Family / organization name" value={author.last_name ?? author.name ?? ""}
              onChange={event => change({ authors: draft.authors!.map((value, i) => i === index ? { ...value, name: undefined, last_name: event.target.value } : value) })} />
            <Input aria-label={`Author ${index + 1} given names`} placeholder="Given names" value={author.fore_name ?? ""}
              onChange={event => change({ authors: draft.authors!.map((value, i) => i === index ? { ...value, fore_name: event.target.value || null } : value) })} />
            <Button type="button" size="icon-sm" variant="ghost" aria-label={`Remove author ${index + 1}`} onClick={() => change({ authors: draft.authors!.filter((_, i) => i !== index) })}><Trash2 /></Button>
          </div>)}
          <Button type="button" size="xs" variant="outline" onClick={() => change({ authors: [...(draft.authors ?? []), { last_name: "", fore_name: "" }] })}><Plus />Add author</Button>
        </fieldset>
        <fieldset className="space-y-2"><legend className="text-xs font-medium">Identifiers</legend>
          {(draft.identifiers ?? []).map((identifier, index) => <div key={index} className="flex items-center gap-2">
            <select aria-label={`Identifier ${index + 1} type`} className="rounded border bg-background p-2" value={identifier.kind}
              onChange={event => change({ identifiers: draft.identifiers!.map((value, i) => i === index ? { ...value, kind: event.target.value } : value) })}>
              {Array.from(new Set([...identifierKinds, identifier.kind])).map(kind => <option key={kind} value={kind}>{kind.toUpperCase()}</option>)}
            </select>
            <Input aria-label={`Identifier ${index + 1} value`} value={identifier.value}
              onChange={event => change({ identifiers: draft.identifiers!.map((value, i) => i === index ? { ...value, value: event.target.value } : value) })} />
            <Button type="button" size="icon-sm" variant="ghost" aria-label={`Remove identifier ${index + 1}`} onClick={() => change({ identifiers: draft.identifiers!.filter((_, i) => i !== index) })}><Trash2 /></Button>
          </div>)}
          <Button type="button" size="xs" variant="outline" onClick={() => change({ identifiers: [...(draft.identifiers ?? []), { kind: "doi", value: "" }] })}><Plus />Add identifier</Button>
        </fieldset>
        <label className="block space-y-1"><span className="text-xs font-medium">Abstract</span>
          <textarea aria-label="Abstract" className={textareaClass} value={draft.abstract_text ?? ""} onChange={event => change({ abstract_text: event.target.value || null })} />
        </label>
        <label className="block space-y-1"><span className="text-xs font-medium">Tags (one per line)</span>
          <textarea aria-label="Tags (one per line)" className={textareaClass} value={keywords} onChange={event => { setKeywords(event.target.value); setDirty(true); }} />
        </label>
      </fieldset>
    </form>
  </SearchDialog>;
}
