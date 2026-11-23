import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { FacetValue, SearchFacets, SearchFilters } from "./bib-search-model";

function Choices({ label, values, selected, onChange }: { label: string; values: FacetValue[]; selected: string[]; onChange: (values: string[]) => void }) {
  const { t } = useTranslation("bibliography");
  const [needle, setNeedle] = useState("");
  const options = [...values, ...selected.filter(id => !values.some(value => value.value === id)).map(value => ({ value, label: t("searchFilters.selectedValue", { label: label.toLowerCase() }), count: 0 }))];
  const matching = options.filter(value => value.label.toLocaleLowerCase().includes(needle.toLocaleLowerCase()));
  return <fieldset className="min-w-0 rounded border p-2"><legend className="px-1 font-medium">{label}</legend>
    <Input className="mb-1 h-7 text-xs" aria-label={t("searchFilters.findAria", { label: label.toLowerCase() })} value={needle} onChange={event => setNeedle(event.target.value)} placeholder={t("searchFilters.findAria", { label: label.toLowerCase() })} />
    <div className="max-h-32 space-y-1 overflow-auto">{matching.slice(0, 100).map(option => <label key={option.value} className="flex items-start gap-1.5"><input type="checkbox" checked={selected.includes(option.value)} onChange={event => onChange(event.target.checked ? [...selected, option.value] : selected.filter(value => value !== option.value))} /><span className="min-w-0 flex-1 break-words">{option.label}</span><span className="text-muted-foreground">{option.count}</span></label>)}</div>
    {!matching.length && <p className="text-muted-foreground">{t("searchFilters.noMatchingOptions")}</p>}
    {matching.length > 100 && <p className="mt-1 text-muted-foreground">{t("searchFilters.first100")}</p>}
  </fieldset>;
}
export function SearchFiltersPanel({ value, facets, onChange }: { value: SearchFilters; facets: SearchFacets | null; onChange: (value: SearchFilters) => void }) {
  const { t } = useTranslation("bibliography");
  const patch = (changes: Partial<SearchFilters>) => onChange({ ...value, ...changes });
  return <section aria-label={t("searchFilters.sectionAria")} className="max-h-[45vh] space-y-2 overflow-auto border-b p-2 text-xs">
    <div className="flex flex-wrap items-center gap-3">
      <label>{t("searchFilters.yearFrom")}<Input className="h-7 w-24" aria-label={t("searchFilters.yearFromAria")} type="number" min="0" max="9999" value={value.year_from ?? ""} onChange={event => patch({ year_from: event.target.value ? Number(event.target.value) : null })} /></label>
      <label>{t("searchFilters.yearTo")}<Input className="h-7 w-24" aria-label={t("searchFilters.yearToAria")} type="number" min="0" max="9999" value={value.year_to ?? ""} onChange={event => patch({ year_to: event.target.value ? Number(event.target.value) : null })} /></label>
      <label>{t("searchFilters.fullText")}<select aria-label={t("searchFilters.fullTextAria")} className="ml-1 rounded border bg-background p-1" value={value.has_fulltext == null ? "any" : String(value.has_fulltext)} onChange={event => patch({ has_fulltext: event.target.value === "any" ? null : event.target.value === "true" })}><option value="any">{t("searchFilters.any")}</option><option value="true">{t("searchFilters.hasFileOrText")}</option><option value="false">{t("searchFilters.noFullText")}</option></select></label>
      <label>{t("searchFilters.notes")}<select aria-label={t("searchFilters.notesAria")} className="ml-1 rounded border bg-background p-1" value={value.has_annotations == null ? "any" : String(value.has_annotations)} onChange={event => patch({ has_annotations: event.target.value === "any" ? null : event.target.value === "true" })}><option value="any">{t("searchFilters.any")}</option><option value="true">{t("searchFilters.hasNotes")}</option><option value="false">{t("searchFilters.noNotes")}</option></select></label>
      <label>{t("searchFilters.textIndexing")}<select aria-label={t("searchFilters.textIndexingAria")} className="ml-1 rounded border bg-background p-1" value={value.fulltext_status ?? ""} onChange={event => patch({ fulltext_status: (event.target.value || null) as SearchFilters["fulltext_status"] })}><option value="">{t("searchFilters.anyState")}</option><option value="indexed">{t("searchFilters.searchableText")}</option><option value="pending">{t("searchFilters.awaitingExtraction")}</option><option value="failed">{t("searchFilters.extractionFailed")}</option><option value="missing">{t("searchFilters.noIndexedFile")}</option></select></label>
      <Button size="xs" variant="ghost" onClick={() => onChange({})}>{t("searchFilters.clearFilters")}</Button>
    </div>
    {facets && <div className="grid gap-2 sm:grid-cols-3">
      <Choices label={t("searchFilters.groupTags")} values={facets.tags} selected={(value.tag_ids ?? []).map(String)} onChange={ids => patch({ tag_ids: ids.map(Number) })} />
      <Choices label={t("searchFilters.groupAuthors")} values={facets.creators} selected={(value.creator_ids ?? []).map(String)} onChange={ids => patch({ creator_ids: ids.map(Number) })} />
      <Choices label={t("searchFilters.groupPublicationTypes")} values={facets.publication_types} selected={value.publication_types ?? []} onChange={types => patch({ publication_types: types })} />
    </div>}
    <p className="text-muted-foreground">{t("searchFilters.hint")}</p>
  </section>;
}
