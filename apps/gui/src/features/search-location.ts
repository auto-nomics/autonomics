import { searchFieldLabels, type SearchExpr, type SearchRequest } from "./bib-search-model";
const object = (value: unknown): value is Record<string, any> => Boolean(value && typeof value === "object" && !Array.isArray(value));
function expression(value: unknown, depth = 0): value is SearchExpr {
  if (!object(value) || depth > 32) return false;
  if (value.op === "and" || value.op === "or") return Array.isArray(value.clauses) && value.clauses.length > 0 && value.clauses.length <= 128 && value.clauses.every((clause: unknown) => expression(clause, depth + 1));
  if (value.op === "not") return expression(value.clause, depth + 1);
  if (value.op === "range") return value.field === "year" && [value.gte, value.lte].every(bound => bound == null || typeof bound === "string");
  return ["term", "phrase"].includes(value.op) && typeof value.value === "string" && value.value.length <= 16384 && (value.field == null || value.field in searchFieldLabels);
}
function validate(value: unknown): value is SearchRequest {
  if (!object(value)) return false;
  if (Object.keys(value).some(key=>!["query","expression","default_fields","scope","filters","sort","page_size","cursor","include_facets","include_articles","sources","mode"].includes(key))) return false;
  if (value.sources != null && (!Array.isArray(value.sources) || !value.sources.every((source:unknown)=>source==="local"))) return false;
  if (value.query != null && (typeof value.query !== "string" || value.query.length > 16384)) return false;
  if (value.expression != null && !expression(value.expression)) return false;
  if (value.sort != null && !["relevance", "title", "newest", "oldest"].includes(value.sort)) return false;
  if (value.mode != null && value.mode !== "lexical") return false;
  if (value.default_fields != null && (!Array.isArray(value.default_fields) || !value.default_fields.every((field: unknown) => typeof field === "string" && field !== "any" && field in searchFieldLabels))) return false;
  if (value.scope != null) {
    const scope = value.scope;
    if (!object(scope) || (scope.collection_id != null && typeof scope.collection_id !== "string") || (scope.library_id != null && (!Number.isSafeInteger(scope.library_id) || scope.library_id <= 0))) return false;
    if ([scope.unfiled, scope.include_descendants, scope.require_same_child].some(flag => flag != null && typeof flag !== "boolean")) return false;
  }
  if (value.filters != null) {
    const filters = value.filters;
    if (!object(filters) || Object.keys(filters).some(key=>!["tag_ids","creator_ids","publication_types","year_from","year_to","has_fulltext","has_annotations","fulltext_status"].includes(key))) return false;
    for (const field of ["tag_ids", "creator_ids"]) if (filters[field] != null && (!Array.isArray(filters[field]) || filters[field].length > 50 || !filters[field].every((id: unknown) => Number.isSafeInteger(id) && Number(id) > 0))) return false;
    if (filters.publication_types != null && (!Array.isArray(filters.publication_types) || !filters.publication_types.every((type: unknown) => typeof type === "string"))) return false;
    for (const field of ["year_from", "year_to"]) if (filters[field] != null && (!Number.isInteger(filters[field]) || filters[field] < 0 || filters[field] > 9999)) return false;
    for (const field of ["has_fulltext", "has_annotations"]) if (filters[field] != null && typeof filters[field] !== "boolean") return false;
    if (filters.fulltext_status != null && !["indexed", "pending", "failed", "missing"].includes(filters.fulltext_status)) return false;
  }
  return true;
}
export function readSearchLocation(): { request: SearchRequest; savedId: string | null; error: string } {
  try {
    const url = new URL(window.location.href);
    const text = url.searchParams.get("bib_search");
    const value = text ? JSON.parse(text) : window.history.state?.bibSearch;
    if (!value) return { request: {}, savedId: null, error: "" };
    if (value.version !== 1 || !validate(value.request)) throw new Error("The search URL is invalid or uses unsupported conditions. Clear it or enter a new query.");
    return { request: value.request, savedId: typeof value.savedId === "string" ? value.savedId : null, error: "" };
  } catch (failure) { return { request: {}, savedId: null, error: String(failure) }; }
}
export function writeSearchLocation(request: SearchRequest, savedId: string | null) {
  const value = { version: 1, request: { ...request, cursor: null }, savedId };
  const text = JSON.stringify(value);
  const url = new URL(window.location.href);
  if (encodeURIComponent(text).length <= 6000) url.searchParams.set("bib_search", text);
  else url.searchParams.delete("bib_search"); // Large trees remain in session history.
  window.history.replaceState({ ...window.history.state, bibSearch: value }, "", url);
}
