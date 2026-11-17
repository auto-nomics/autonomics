//! OpenAPI contract for the bibliography HTTP module.
//! Paths include their gateway mount prefix, `/api/v1/bib`.
use utoipa::{Modify, OpenApi};

#[derive(utoipa::ToSchema)]
#[allow(dead_code)] // Schema-only DTO: the actual handler reads multipart fields.
pub(crate) struct AttachmentUpload {
    /// Original file bytes (maximum request body: 256 MiB).
    #[schema(value_type = String, format = Binary)]
    file: Vec<u8>,
    /// Optional display title.
    title: Option<String>,
    /// Whether this becomes the preferred attachment (default true for HTTP uploads).
    preferred: Option<bool>,
}

#[derive(utoipa::ToSchema)]
#[allow(dead_code)] // Schema-only multipart DTO.
pub(crate) struct FulltextUpload {
    #[schema(value_type = String, format = Binary)]
    file: Vec<u8>,
}

// These envelopes describe handlers that return dynamic JSON values.
#[derive(utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct ArticleListResponse {
    total: usize,
    articles: Vec<bib_types::Article>,
    associations: Vec<serde_json::Value>,
    attachment_summaries: serde_json::Value,
}

#[derive(utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct ArticleDetailResponse {
    revision: String,
    article: bib_types::Article,
    attachments: Vec<bib_base::StoredAttachment>,
    fulltext: Option<bib_types::FullText>,
    fulltext_pagination: Option<serde_json::Value>,
    annotations: Vec<bib_types::Annotation>,
}

#[derive(utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct DiscoverySessionResponse {
    session: bib_types::discovery::DiscoverySessionSnapshot,
}

#[derive(utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct DiscoveryPreviewResponse {
    preview: bib_types::discovery::DiscoveryImportPreview,
}

#[derive(utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct DiscoveryImportResponse {
    items: Vec<bib_types::discovery::DiscoveryImportOutcome>,
    imported: usize,
    failed: usize,
    acquisition: Option<bib_types::discovery::DiscoveryAcquisitionJob>,
}

#[derive(utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct AcquisitionResponse {
    acquisition: bib_types::discovery::DiscoveryAcquisitionJob,
}

#[derive(utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct AttachmentResponse {
    attachment: bib_base::StoredAttachment,
}

#[derive(utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct AttachmentListResponse {
    attachments: Vec<bib_base::StoredAttachment>,
}

#[derive(utoipa::ToSchema)]
#[allow(dead_code)]
pub(crate) struct CollectionPatch {
    /// Omit to retain the current name.
    name: Option<String>,
    /// Omit to retain the parent; null moves the collection to the root.
    parent_id: Option<String>,
}

#[derive(OpenApi)]
#[openapi(
    modifiers(&BibliographyMetadata),
    paths(
        crate::bib::health,
        crate::bib::list_articles,
        crate::bib::create_article,
        crate::bib::create_manual_article,
        crate::bib::list_article_trash,
        crate::bib::restore_article,
        crate::bib::permanently_delete_article,
        crate::bib::list_unfiled_articles,
        crate::bib::import_article,
        crate::bib::get_article,
        crate::bib::update_article,
        crate::bib::delete_article,
        crate::bib::list_attachments,
        crate::bib::upload_attachment,
        crate::bib::get_attachment,
        crate::bib::delete_attachment_by_key,
        crate::bib::set_preferred_attachment,
        crate::bib::list_attachment_annotations,
        crate::bib::add_attachment_annotation,
        crate::bib::get_attachment_fulltext,
        crate::bib::reextract_attachment_fulltext,
        crate::bib::download_attachment,
        crate::bib::get_fulltext,
        crate::bib::upload_fulltext,
        crate::bib::delete_fulltext,
        crate::bib::get_structured_fulltext,
        crate::bib::generate_structured_fulltext,
        crate::bib::get_attachment_structure,
        crate::bib::generate_attachment_structure,
        crate::bib::download_fulltext,
        crate::bib::download_figure,
        crate::bib::reextract_fulltext,
        crate::bib::list_annotations,
        crate::bib::add_annotation,
        crate::bib::delete_annotation,
        crate::bib::update_annotation,
        crate::bib::list_collections,
        crate::bib::create_collection,
        crate::bib::get_collection,
        crate::bib::delete_collection,
        crate::bib::update_collection,
        crate::bib::list_collection_articles,
        crate::bib::add_to_collection,
        crate::bib::remove_from_collection,
        crate::bib::list_top_collections,
        crate::bib::collection_changes,
        crate::bib::list_trashed_collections,
        crate::bib::list_child_collections,
        crate::bib::restore_collection,
        crate::bib::update_collection_status,
        crate::bib::list_requests,
        crate::bib::export,
        crate::bib::search_external,
        crate::bib::search_local_v2,
        crate::bib::explore_literature,
        crate::bib::parse_search_query,
        crate::bib::cancel_search_request,
        crate::bib::export_search,
        crate::bib::search_evidence,
        crate::bib::list_saved_searches,
        crate::bib::create_saved_search,
        crate::bib::update_saved_search,
        crate::bib::delete_saved_search,
        crate::bib::search_index_status,
        crate::bib::semantic_search_status,
        crate::bib::configure_semantic_search,
        crate::bib::rebuild_search_index,
        crate::bib::semantic_models,
        crate::bib::semantic_maintenance,
        crate::discovery::sources,
        crate::discovery::create,
        crate::discovery::snapshot,
        crate::discovery::remove,
        crate::discovery::events,
        crate::discovery::results,
        crate::discovery::candidate,
        crate::discovery::cancel,
        crate::discovery::next,
        crate::discovery::retry,
        crate::discovery::import_preview,
        crate::discovery::import,
        crate::discovery::provenance,
        crate::discovery::acquisition,
        crate::discovery::cancel_acquisition,
        crate::bib_transfer::preview,
        crate::bib_transfer::commit,
        crate::bib_transfer::render,
        crate::bib_transfer::duplicates,
        crate::bib_transfer::merge,
        crate::rss::list_feeds,
        crate::rss::create_feed,
        crate::rss::status,
        crate::rss::read_feed,
        crate::rss::replace_feed,
        crate::rss::delete_feed,
        crate::rss::check_feed,
        crate::rss::set_enabled,
        crate::rss::read_item,
        crate::rss::set_item_read,
        crate::rss::delete_item,
        crate::rss::import_item,
        crate::rss::items,
    ),
    tags(
        (name = "bibliography", description = "Literature records and recycle bin"),
        (name = "bibliography-attachments", description = "Attachments, full text, extraction and downloads"),
        (name = "bibliography-annotations", description = "Article and attachment annotations"),
        (name = "bibliography-collections", description = "Collections, hierarchy and memberships"),
        (name = "bibliography-search", description = "Local, semantic and external search; saved searches"),
        (name = "literature-discovery", description = "Search sessions, candidate import and full-text acquisition jobs"),
        (name = "bibliography-import-export", description = "File import, citation rendering and duplicate merging"),
        (name = "bibliography-rss", description = "RSS subscriptions and feed inbox"),
    )
)]
pub struct BibliographyApiDoc;

struct BibliographyMetadata;

impl Modify for BibliographyMetadata {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        // The GET and HEAD routes share a handler but need distinct operation IDs.
        for item in openapi.paths.paths.values_mut() {
            if let Some(head) = &mut item.head
                && let Some(id) = &mut head.operation_id
            {
                id.push_str("_head");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use syn::visit::Visit;

    const METHODS: &[&str] = &[
        "get", "post", "put", "patch", "delete", "head", "options", "trace",
    ];

    #[derive(Default)]
    struct RouteMethods(BTreeSet<String>);
    impl<'ast> Visit<'ast> for RouteMethods {
        fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
            if let syn::Expr::Path(path) = &*node.func {
                let name = path.path.segments.last().unwrap().ident.to_string();
                if METHODS.contains(&name.as_str()) {
                    self.0.insert(name);
                }
            }
            syn::visit::visit_expr_call(self, node);
        }
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            let name = node.method.to_string();
            if METHODS.contains(&name.as_str()) {
                self.0.insert(name);
            }
            syn::visit::visit_expr_method_call(self, node);
        }
    }

    #[derive(Default)]
    struct Routes(BTreeSet<(String, String)>);
    impl<'ast> Visit<'ast> for Routes {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            if node.method == "route" {
                let syn::Expr::Lit(path) = &node.args[0] else {
                    panic!("route must have a literal path")
                };
                let syn::Lit::Str(path) = &path.lit else {
                    panic!("route path must be a string")
                };
                let mut methods = RouteMethods::default();
                methods.visit_expr(&node.args[1]);
                for method in methods.0 {
                    self.0
                        .insert((format!("/api/v1/bib{}", path.value()), method));
                }
            }
            syn::visit::visit_expr_method_call(self, node);
        }
    }

    #[test]
    fn every_bibliography_route_is_documented_with_the_correct_method() {
        let mut routes = Routes::default();
        for source in [
            include_str!("bib.rs"),
            include_str!("discovery.rs"),
            include_str!("bib_transfer.rs"),
            include_str!("rss.rs"),
        ] {
            let file = syn::parse_file(source).unwrap();
            let router = file
                .items
                .iter()
                .find_map(|item| match item {
                    syn::Item::Fn(function) if function.sig.ident == "router" => Some(function),
                    _ => None,
                })
                .unwrap();
            routes.visit_block(&router.block);
        }
        let doc = serde_json::to_value(BibliographyApiDoc::openapi()).unwrap();
        let mut documented = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for (path, item) in doc["paths"].as_object().unwrap() {
            for (method, operation) in item.as_object().unwrap() {
                if !METHODS.contains(&method.as_str()) {
                    continue;
                }
                documented.insert((path.clone(), method.clone()));
                assert!(
                    ids.insert(operation["operationId"].as_str().unwrap()),
                    "duplicate operationId at {method} {path}"
                );
                for segment in path.split('/').filter(|s| s.starts_with('{')) {
                    let name = segment.trim_matches(['{', '}']);
                    assert!(
                        operation["parameters"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|p| p["name"] == name
                                && p["in"] == "path"
                                && p["required"] == true),
                        "missing required path parameter {name} in {method} {path}"
                    );
                }
            }
        }
        assert_eq!(documented, routes.0);
    }

    fn assert_references(value: &serde_json::Value, root: &serde_json::Value) {
        match value {
            serde_json::Value::Object(fields) => {
                if let Some(reference) = fields.get("$ref").and_then(serde_json::Value::as_str) {
                    assert!(
                        root.pointer(reference.strip_prefix('#').unwrap()).is_some(),
                        "unresolved schema: {reference}"
                    );
                }
                for child in fields.values() {
                    assert_references(child, root);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    assert_references(item, root);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn schemas_uploads_and_query_parameters_are_usable() {
        let doc = serde_json::to_value(BibliographyApiDoc::openapi()).unwrap();
        assert_references(&doc, &doc);
        let upload = &doc["paths"]["/api/v1/bib/articles/{id}/attachments"]["post"]["requestBody"]
            ["content"];
        assert!(upload.get("multipart/form-data").is_some());
        assert_eq!(
            doc["components"]["schemas"]["AttachmentUpload"]["properties"]["file"]["format"],
            "binary"
        );
        assert!(doc["components"]["schemas"]["Article"]["properties"]["title"].is_object());
        let events = &doc["paths"]["/api/v1/bib/search/sessions/{id}/events"]["get"];
        assert!(
            events["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["name"] == "wait_ms" && p["in"] == "query")
        );
        assert!(
            doc["paths"]["/api/v1/bib/rss/feeds"]["get"]
                .get("requestBody")
                .is_none()
        );
    }
}
