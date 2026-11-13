//! Retrieval scope accompanies catalogue evidence without changing result assets.
use serde_json::{Value, json};
// Native SearchPage values always serialize as objects; callers retain their
// existing assets/count/pagination fields at the top level for compatibility.
pub(crate) fn with_scope(provider: &str, query: &str, mut page: Value) -> Value {
    page["search_scope"] = json!({
        "provider": provider,
        "query": query,
        "coverage": "submitted keywords against the available provider index; not a semantic census of the catalogue",
        "scientific_suitability_assessed": false,
        "proves_catalogue_absence": false
    });
    if let Some(author) = page.get("author_filter").cloned() {
        page["search_scope"]["author_filter"] = author;
    }
    page
}
