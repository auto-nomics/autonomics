import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { FacetValue, SearchFacets, SearchFilters } from "./bib-search-model";

function Choices({ label, values, selected, onChange }: { label: string; values: FacetValue[]; selected: string[]; onChange: (values: string[]) => void }) {
  const [needle, setNeedle] = useState("");
  const options = [...values, ...selected.filter(id => !values.some(value => value.value === id)).map(value => ({ value, label: `Selected ${label.toLowerCase()}`, count: 0 }))];
  const matching = options.filter(value => value.label.toLocaleLowerCase().includes(needle.toLocaleLowerCase()));
  return <fieldset className="min-w-0 rounded border p-2"><legend className="px-1 font-medium">{label}</legend>
    <Input className="mb-1 h-7 text-xs" aria-label={`Find ${label.toLowerCase()}`} value={needle} onChange={event => setNeedle(event.target.value)} placeholder={`Find ${label.toLowerCase()}`} />
    <div className="max-h-32 space-y-1 overflow-auto">{matching.slice(0, 100).map(option => <label key={option.value} className="flex items-start gap-1.5"><input type="checkbox" checked={selected.includes(option.value)} onChange={event => onChange(event.target.checked ? [...selected, option.value] : selected.filter(value => value !== option.value))} /><span className="min-w-0 flex-1 break-words">{option.label}</span><span className="text-muted-foreground">{option.count}</span></label>)}</div>
    {!matching.length && <p className="text-muted-foreground">No matching options</p>}
    {matching.length > 100 && <p className="mt-1 text-muted-foreground">Showing the first 100 options. Type to narrow the list.</p>}
  </fieldset>;
}
export function SearchFiltersPanel({ value, facets, onChange }: { value: SearchFilters; facets: SearchFacets | null; onChange: (value: SearchFilters) => void }) {
  const patch = (changes: Partial<SearchFilters>) => onChange({ ...value, ...changes });
  return <section aria-label="Search filters" className="max-h-[45vh] space-y-2 overflow-auto border-b p-2 text-xs">
    <div className="flex flex-wrap items-center gap-3">
      <label>Year from<Input className="h-7 w-24" aria-label="Filter year from" type="number" min="0" max="9999" value={value.year_from ?? ""} onChange={event => patch({ year_from: event.target.value ? Number(event.target.value) : null })} /></label>
      <label>Year to<Input className="h-7 w-24" aria-label="Filter year to" type="number" min="0" max="9999" value={value.year_to ?? ""} onChange={event => patch({ year_to: event.target.value ? Number(event.target.value) : null })} /></label>
      <label>Full text<select aria-label="Full text availability" className="ml-1 rounded border bg-background p-1" value={value.has_fulltext == null ? "any" : String(value.has_fulltext)} onChange={event => patch({ has_fulltext: event.target.value === "any" ? null : event.target.value === "true" })}><option value="any">Any</option><option value="true">Has file or text</option><option value="false">No full text</option></select></label>
      <label>Notes<select aria-label="Annotation availability" className="ml-1 rounded border bg-background p-1" value={value.has_annotations == null ? "any" : String(value.has_annotations)} onChange={event => patch({ has_annotations: event.target.value === "any" ? null : event.target.value === "true" })}><option value="any">Any</option><option value="true">Has notes</option><option value="false">No notes</option></select></label>
      <label>Text indexing<select aria-label="Full text index state" className="ml-1 rounded border bg-background p-1" value={value.fulltext_status ?? ""} onChange={event => patch({ fulltext_status: (event.target.value || null) as SearchFilters["fulltext_status"] })}><option value="">Any state</option><option value="indexed">Searchable text</option><option value="pending">Awaiting extraction</option><option value="failed">Extraction failed</option><option value="missing">No indexed file</option></select></label>
      <Button size="xs" variant="ghost" onClick={() => onChange({})}>Clear filters</Button>
    </div>
    {facets && <div className="grid gap-2 sm:grid-cols-3">
      <Choices label="Tags" values={facets.tags} selected={(value.tag_ids ?? []).map(String)} onChange={ids => patch({ tag_ids: ids.map(Number) })} />
      <Choices label="Authors" values={facets.creators} selected={(value.creator_ids ?? []).map(String)} onChange={ids => patch({ creator_ids: ids.map(Number) })} />
      <Choices label="Publication types" values={facets.publication_types} selected={value.publication_types ?? []} onChange={types => patch({ publication_types: types })} />
    </div>}
    <p className="text-muted-foreground">Choose any value within a group; all groups must match. Counts show articles before that group’s own selection. An article can have both searchable and failed attachments.</p>
  </section>;
}
