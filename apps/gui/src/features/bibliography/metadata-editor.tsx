import { useRef, useState } from "react";
import { ArrowDown, ArrowUp, LoaderCircle, Plus, Save, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { gatewayRequest } from "@/gateway";
import type { BibArticle } from "@/types";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SearchDialog } from "./search-dialog";

const textFields = ["title", "journal", "volume", "issue", "pages", "language", "issn", "essn"] as const;
export const extraFieldLabels: Record<string, string> = {
  publisher: "Publisher", place: "Place", edition: "Edition", book_title: "Book title",
  university: "University", report_type: "Report type", institution: "Institution",
  patent_number: "Patent number", website_title: "Website title", access_date: "Access date", url: "URL",
};
const typeFields: Record<string, string[]> = {
  Book: ["publisher", "place", "edition"], "Book Section": ["book_title", "publisher", "place", "edition"],
  Thesis: ["university", "place"], Report: ["institution", "report_type", "place"],
  Patent: ["patent_number", "institution", "place"], "Web Page": ["website_title"],
  "Conference Paper": ["book_title", "publisher", "place"], Dataset: ["publisher"],
};
const identifierKinds = ["doi", "pmid", "pmc", "arxiv", "embase", "biorxiv", "s2", "openalex", "other"];
const textareaClass = "min-h-24 w-full rounded-md border bg-background px-3 py-2 text-sm";

export function MetadataEditor({ article, revision, create = false, onClose, onSaved }: {
  article: BibArticle;
  revision?: string;
  create?: boolean;
  onClose: () => void;
  onSaved: (article: BibArticle, revision?: string) => void;
}) {
  const { t } = useTranslation("bibliography");
  const extraFieldLabel = (field: string) => t(`metadataEditor.extraFields.${field}`, { defaultValue: extraFieldLabels[field] ?? field });
  // Keep the complete detail object: PUT replaces the record, including fields not shown here.
  const [draft, setDraft] = useState(() => ({ ...article }));
  const [currentRevision, setCurrentRevision] = useState(revision);
  const [conflict, setConflict] = useState(false);
  const [year, setYear] = useState(String(article.year ?? ""));
  const [keywords, setKeywords] = useState((article.keywords ?? []).join("\n"));
  const [dirty, setDirty] = useState(create);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const saving = useRef(false);
  const change = (patch: Partial<BibArticle>) => { setDraft(current => ({ ...current, ...patch })); setDirty(true); };
  const close = () => {
    if (!saving.current && (!dirty || window.confirm(t("metadataEditor.discardConfirm")))) onClose();
  };
  const save = async () => {
    if (saving.current) return;
    if (!draft.title.trim()) { setError(t("metadataEditor.titleRequired")); return; }
    if (year.trim() && (!/^\d{1,4}$/.test(year.trim()) || Number(year) < 1)) {
      setError(t("metadataEditor.yearInvalid")); return;
    }
    if ((draft.authors ?? []).some(author => !author.last_name?.trim() && !author.name?.trim())) {
      setError(t("metadataEditor.authorNameRequired")); return;
    }
    if ((draft.identifiers ?? []).some(identifier => !identifier.value.trim())) {
      setError(t("metadataEditor.identifierValueRequired")); return;
    }
    if (draft.extra_fields?.url) {
      try { if (!["http:", "https:"].includes(new URL(draft.extra_fields.url).protocol)) throw new Error(); }
      catch { setError(t("metadataEditor.urlInvalid")); return; }
    }
    const accessDate = draft.extra_fields?.access_date;
    if (accessDate && (!/^\d{4}-\d{2}-\d{2}$/.test(accessDate) || Number.isNaN(Date.parse(accessDate)) || new Date(accessDate).toISOString().slice(0, 10) !== accessDate)) {
      setError(t("metadataEditor.accessDateInvalid")); return;
    }
    saving.current = true; setBusy(true); setError(null);
    try {
      const response = await gatewayRequest(create ? "POST" : "PUT", create ? "/bib/articles/manual" : `/bib/articles/${encodeURIComponent(article.id)}`, {
        ...(currentRevision ? { expected_revision: currentRevision } : {}),
        ...draft, title: draft.title.trim(), year: year.trim() ? Number(year) : null,
        authors: (draft.authors ?? []).map(author => ({ ...author, last_name: (author.last_name || author.name || "").trim() })),
        identifiers: (draft.identifiers ?? []).map(identifier => ({ ...identifier, value: identifier.value.trim() })),
        keywords: [...new Set(keywords.split(/\r?\n/).map(value => value.trim()).filter(Boolean))],
      });
      onSaved(response.article, response.revision);
    } catch (failure) {
      setConflict(/record changed|after editing|revision/i.test(String(failure)));
      setError(t("metadataEditor.saveFailed", { error: String(failure) }));
    } finally { saving.current = false; setBusy(false); }
  };
  const reload = async () => {
    if (!window.confirm(t("metadataEditor.reloadConfirm"))) return;
    setBusy(true);
    try {
      const latest = await gatewayRequest("GET", `/bib/articles/${encodeURIComponent(article.id)}`);
      setDraft(latest.article); setCurrentRevision(latest.revision); setYear(String(latest.article.year ?? ""));
      setKeywords((latest.article.keywords ?? []).join("\n")); setDirty(false); setConflict(false); setError(null);
    } catch (failure) { setError(String(failure)); }
    finally { setBusy(false); }
  };
  const moveAuthor = (index: number, delta: number) => {
    const authors = [...(draft.authors ?? [])];
    [authors[index], authors[index + delta]] = [authors[index + delta], authors[index]];
    change({ authors });
  };
  return <SearchDialog title={create ? t("metadataEditor.newTitle") : t("metadataEditor.editTitle")} onClose={close} footer={
    <div className="flex items-center justify-end gap-2">
      <Button variant="ghost" disabled={busy} onClick={close}>{t("metadataEditor.cancel")}</Button>
      <Button disabled={busy || !dirty} onClick={() => void save()}>
        {busy ? <LoaderCircle className="animate-spin" /> : <Save />} {busy ? t("metadataEditor.saving") : t("metadataEditor.saveChanges")}
      </Button>
    </div>
  }>
    <form onSubmit={event => { event.preventDefault(); void save(); }}>
      {error && <div role="alert" className="mb-3 rounded border border-destructive/40 p-2 text-sm text-destructive">{error}{conflict && <div className="mt-2"><p>{t("metadataEditor.conflictNotice")}</p><Button size="xs" variant="outline" disabled={busy} onClick={() => void reload()}>{t("metadataEditor.loadLatest")}</Button></div>}</div>}
      <fieldset disabled={busy} className="space-y-4">
        <label className="block space-y-1"><span className="text-xs font-medium">{t("metadataEditor.itemType")}</span>
          <select aria-label={t("metadataEditor.itemType")} className="block w-full rounded border bg-background p-2" value={draft.pub_types?.[0] ?? "Journal Article"}
            onChange={event => change({ pub_types: [event.target.value, ...(draft.pub_types ?? []).slice(1)] })}>
            {Array.from(new Set(["Journal Article", "Book", "Book Section", "Thesis", "Report", "Patent", "Web Page", "Conference Paper", "Dataset", ...(draft.pub_types ?? [])])).map(type => <option key={type}>{type}</option>)}
          </select>
        </label>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          {textFields.map(field => <label key={field} className={field === "title" ? "space-y-1 sm:col-span-2" : "space-y-1"}>
            <span className="text-xs font-medium">{t(`metadataEditor.fields.${field}`)}{field === "title" ? " *" : ""}</span>
            <Input aria-label={t(`metadataEditor.fields.${field}`)} value={draft[field] ?? ""} onChange={event => change({ [field]: (field === "title" ? event.target.value : event.target.value || null) })} />
          </label>)}
          <label className="space-y-1"><span className="text-xs font-medium">{t("metadataEditor.fields.year")}</span>
            <Input aria-label={t("metadataEditor.fields.year")} inputMode="numeric" value={year} onChange={event => { setYear(event.target.value); setDirty(true); }} />
          </label>
        </div>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          {[...(typeFields[draft.pub_types?.[0] ?? "Journal Article"] ?? []), "url", "access_date"].map(field => <label key={field} className="space-y-1">
            <span className="text-xs font-medium">{extraFieldLabel(field)}</span>
            <Input aria-label={extraFieldLabel(field)} value={draft.extra_fields?.[field] ?? ""} placeholder={field === "access_date" ? "YYYY-MM-DD" : undefined}
              onChange={event => { const extra_fields = { ...draft.extra_fields }; if (event.target.value) extra_fields[field] = event.target.value; else delete extra_fields[field]; change({ extra_fields }); }} />
          </label>)}
        </div>
        <fieldset className="space-y-2"><legend className="text-xs font-medium">{t("metadataEditor.authorsLegend")}</legend>
          {(draft.authors ?? []).map((author, index) => <div key={index} className="flex items-center gap-2">
            <Input aria-label={t("metadataEditor.authorFamilyAria", { index: index + 1 })} placeholder={t("metadataEditor.familyPlaceholder")} value={author.last_name ?? author.name ?? ""}
              onChange={event => change({ authors: draft.authors!.map((value, i) => i === index ? { ...value, name: undefined, last_name: event.target.value } : value) })} />
            <Input aria-label={t("metadataEditor.authorGivenAria", { index: index + 1 })} placeholder={t("metadataEditor.givenPlaceholder")} value={author.fore_name ?? ""}
              onChange={event => change({ authors: draft.authors!.map((value, i) => i === index ? { ...value, fore_name: event.target.value || null } : value) })} />
            <Button type="button" size="icon-xs" variant="ghost" disabled={index === 0} aria-label={t("metadataEditor.moveAuthorUpAria", { index: index + 1 })} onClick={() => moveAuthor(index, -1)}><ArrowUp /></Button>
            <Button type="button" size="icon-xs" variant="ghost" disabled={index === (draft.authors?.length ?? 0) - 1} aria-label={t("metadataEditor.moveAuthorDownAria", { index: index + 1 })} onClick={() => moveAuthor(index, 1)}><ArrowDown /></Button>
            <Button type="button" size="icon-sm" variant="ghost" aria-label={t("metadataEditor.removeAuthorAria", { index: index + 1 })} onClick={() => change({ authors: draft.authors!.filter((_, i) => i !== index) })}><Trash2 /></Button>
          </div>)}
          <Button type="button" size="xs" variant="outline" onClick={() => change({ authors: [...(draft.authors ?? []), { last_name: "", fore_name: "" }] })}><Plus />{t("metadataEditor.addAuthor")}</Button>
        </fieldset>
        <fieldset className="space-y-2"><legend className="text-xs font-medium">{t("metadataEditor.identifiersLegend")}</legend>
          {(draft.identifiers ?? []).map((identifier, index) => <div key={index} className="flex items-center gap-2">
            <select aria-label={t("metadataEditor.identifierTypeAria", { index: index + 1 })} className="rounded border bg-background p-2" value={identifier.kind}
              onChange={event => change({ identifiers: draft.identifiers!.map((value, i) => i === index ? { ...value, kind: event.target.value } : value) })}>
              {Array.from(new Set([...identifierKinds, identifier.kind])).map(kind => <option key={kind} value={kind}>{kind.toUpperCase()}</option>)}
            </select>
            <Input aria-label={t("metadataEditor.identifierValueAria", { index: index + 1 })} value={identifier.value}
              onChange={event => change({ identifiers: draft.identifiers!.map((value, i) => i === index ? { ...value, value: event.target.value } : value) })} />
            <Button type="button" size="icon-sm" variant="ghost" aria-label={t("metadataEditor.removeIdentifierAria", { index: index + 1 })} onClick={() => change({ identifiers: draft.identifiers!.filter((_, i) => i !== index) })}><Trash2 /></Button>
          </div>)}
          <Button type="button" size="xs" variant="outline" onClick={() => change({ identifiers: [...(draft.identifiers ?? []), { kind: "doi", value: "" }] })}><Plus />{t("metadataEditor.addIdentifier")}</Button>
        </fieldset>
        <label className="block space-y-1"><span className="text-xs font-medium">{t("metadataEditor.abstract")}</span>
          <textarea aria-label={t("metadataEditor.abstract")} className={textareaClass} value={draft.abstract_text ?? ""} onChange={event => change({ abstract_text: event.target.value || null })} />
        </label>
        <label className="block space-y-1"><span className="text-xs font-medium">{t("metadataEditor.tags")}</span>
          <textarea aria-label={t("metadataEditor.tags")} className={textareaClass} value={keywords} onChange={event => { setKeywords(event.target.value); setDirty(true); }} />
        </label>
      </fieldset>
    </form>
  </SearchDialog>;
}
