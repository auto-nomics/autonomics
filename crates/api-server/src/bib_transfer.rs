use super::bib_transfer_parser::parse;
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use bib_base::BibShared;
use bib_types::{Article, ExportFormat};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
type ResultJson = Result<Json<Value>, (StatusCode, Json<Value>)>;
fn bad(message: impl ToString) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error":message.to_string()})),
    )
}
fn internal(message: impl ToString) -> (StatusCode, Json<Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error":message.to_string()})),
    )
}
pub(crate) fn router() -> Router<Arc<BibShared>> {
    Router::new()
        .route("/import/preview", post(preview))
        .route("/import/commit", post(commit))
        .route("/citations/render", post(render))
        .route("/duplicates", get(duplicates))
        .route("/duplicates/merge", post(merge))
}
#[derive(Deserialize, utoipa::ToSchema)]
struct ImportInput {
    format: String,
    contents: String,
    selected_indices: Option<Vec<usize>>,
    collection_id: Option<String>,
}
fn keys(article: &Article) -> Vec<String> {
    bib_base::library_merge::library_identity_keys(article)
}
#[utoipa::path(method(post), path = "/api/v1/bib/import/preview", tag = "bibliography-import-export",
    operation_id = "bib_bib_transfer_preview",
    summary = "Preview",
    request_body = ImportInput,
    responses((status = 200, description = "Preview", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn preview(
    State(shared): State<Arc<BibShared>>,
    Json(input): Json<ImportInput>,
) -> ResultJson {
    let records = parse(&input.format, &input.contents).map_err(bad)?;
    let existing = shared.bib.list_all_articles().await.map_err(internal)?;
    let mut by_key = HashMap::<String, HashSet<String>>::new();
    for article in &existing {
        for key in keys(article) {
            by_key.entry(key).or_default().insert(article.id.clone());
        }
    }
    let by_id: HashSet<_> = existing.iter().map(|a| a.id.as_str()).collect();
    let mut seen = HashMap::<String, usize>::new();
    let mut rows = Vec::new();
    for (index, record) in records.into_iter().enumerate() {
        let mut matches = HashSet::new();
        if by_id.contains(record.article.id.as_str()) {
            matches.insert(record.article.id.clone());
        }
        for key in keys(&record.article) {
            if let Some(ids) = by_key.get(&key) {
                matches.extend(ids.iter().cloned());
            }
        }
        if matches.len() > 1 {
            return Err(bad(format!(
                "Record {} has identifiers matching multiple existing records; resolve the conflict before importing",
                index + 1
            )));
        }
        let duplicate_id = matches.into_iter().next();
        let mut within = None;
        for key in keys(&record.article)
            .into_iter()
            .chain(std::iter::once(record.article.id.clone()))
        {
            if let Some(previous) = seen.get(&key) {
                within = Some(*previous);
            } else {
                seen.insert(key, index);
            }
        }
        rows.push(json!({"index":index,"article":record.article,"warnings":record.warnings,"duplicate_id":duplicate_id,"duplicate_in_file":within}));
    }
    Ok(Json(json!({"records":rows,"total":rows.len()})))
}
#[utoipa::path(method(post), path = "/api/v1/bib/import/commit", tag = "bibliography-import-export",
    operation_id = "bib_bib_transfer_commit",
    summary = "Commit",
    request_body = ImportInput,
    responses((status = 200, description = "Commit", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn commit(
    State(shared): State<Arc<BibShared>>,
    Json(input): Json<ImportInput>,
) -> ResultJson {
    let records = parse(&input.format, &input.contents).map_err(bad)?;
    let indices = input
        .selected_indices
        .ok_or_else(|| bad("Preview and select records before importing"))?;
    if indices.is_empty() || indices.iter().any(|i| *i >= records.len()) {
        return Err(bad("Invalid or empty record selection"));
    }
    let mut seen = HashSet::new();
    let articles = indices
        .into_iter()
        .filter(|i| seen.insert(*i))
        .map(|i| records[i].article.clone())
        .collect();
    let outcome = shared
        .bib
        .import_library_records(articles, input.collection_id)
        .await
        .map_err(bad)?;
    Ok(Json(json!(outcome)))
}
#[derive(Deserialize, utoipa::ToSchema)]
struct RenderInput {
    article_ids: Vec<String>,
    format: String,
}
#[utoipa::path(method(post), path = "/api/v1/bib/citations/render", tag = "bibliography-import-export",
    operation_id = "bib_bib_transfer_render",
    summary = "Render",
    request_body = RenderInput,
    responses((status = 200, description = "Render", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn render(
    State(shared): State<Arc<BibShared>>,
    Json(input): Json<RenderInput>,
) -> ResultJson {
    if input.article_ids.is_empty() || input.article_ids.len() > 2000 {
        return Err(bad("Select 1–2000 records"));
    }
    let format = match input.format.as_str() {
        "bibtex" => ExportFormat::Bibtex,
        "ris" => ExportFormat::Ris,
        "csl_json" => ExportFormat::CslJson,
        "markdown" => ExportFormat::Markdown,
        _ => return Err(bad("Unknown export format")),
    };
    let mut articles = Vec::new();
    let mut seen = HashSet::new();
    for id in input.article_ids {
        if seen.insert(id.clone()) {
            articles.push(
                shared
                    .bib
                    .get_article(&id)
                    .await
                    .map_err(internal)?
                    .ok_or_else(|| bad(format!("Article {id} is unavailable")))?,
            );
        }
    }
    let items: Vec<_> = articles.iter().map(bib_base::to_csl_json).collect();
    let output = bib_base::render_all(&articles, format);
    Ok(Json(json!({"export":output,"items":items})))
}
fn normalized_title(article: &Article) -> String {
    article
        .title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
#[utoipa::path(method(get), path = "/api/v1/bib/duplicates", tag = "bibliography-import-export",
    operation_id = "bib_bib_transfer_duplicates",
    summary = "Duplicates",
    responses((status = 200, description = "Duplicates", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn duplicates(State(shared): State<Arc<BibShared>>) -> ResultJson {
    let articles = shared.bib.list_all_articles().await.map_err(internal)?;
    // Bounded review sets: title matches are suggestions, never automatic merges.
    let mut buckets: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, article) in articles.iter().enumerate() {
        for key in keys(article) {
            buckets.entry(key).or_default().push(index);
        }
        let title = normalized_title(article);
        if title.len() > 8 {
            buckets
                .entry(format!("title:{title}"))
                .or_default()
                .push(index);
        }
    }
    let mut seen = HashSet::new();
    let mut groups = Vec::new();
    for (reason, mut indices) in buckets {
        indices.sort_unstable();
        indices.dedup();
        if indices.len() < 2 || !seen.insert(indices.clone()) {
            continue;
        }
        groups.push(json!({"reason":reason,"articles":indices.iter().map(|i|&articles[*i]).collect::<Vec<_>>(),"revisions":indices.iter().map(|i|(articles[*i].id.clone(),bib_base::article_revision(&articles[*i]))).collect::<HashMap<_,_>>()}));
    }
    groups.sort_by_key(|v| v["reason"].as_str().unwrap_or_default().to_owned());
    let total = groups.len();
    groups.truncate(200);
    Ok(Json(
        json!({"groups":groups,"total":total,"truncated":total>200}),
    ))
}
#[derive(Deserialize, utoipa::ToSchema)]
struct MergeInput {
    target_id: String,
    source_ids: Vec<String>,
    expected_revisions: HashMap<String, String>,
    metadata: Option<Article>,
}
#[utoipa::path(method(post), path = "/api/v1/bib/duplicates/merge", tag = "bibliography-import-export",
    operation_id = "bib_bib_transfer_merge",
    summary = "Merge",
    request_body = MergeInput,
    responses((status = 200, description = "Merge", body = Value, content_type = "application/json"), (status = 400, description = "Invalid input"), (status = 401, description = "Authentication required when AUTONOMICS_HTTP_API_TOKEN is set"), (status = 404, description = "Record, attachment, collection or session not found"), (status = 409, description = "Revision, fingerprint or identity conflict")),
)]
async fn merge(State(shared): State<Arc<BibShared>>, Json(input): Json<MergeInput>) -> ResultJson {
    if input.source_ids.is_empty()
        || input.source_ids.len() > 100
        || input.source_ids.contains(&input.target_id)
    {
        return Err(bad("Invalid merge selection"));
    }
    let asset_revisions = shared
        .bib
        .library_merge_asset_revisions(input.source_ids.clone())
        .await
        .map_err(internal)?;
    preserve_merge_assets(&shared, &input).await?;
    let article = shared
        .bib
        .merge_library_articles_checked(
            input.target_id,
            input.source_ids,
            input.expected_revisions,
            input.metadata,
            Some(asset_revisions),
        )
        .await
        .map_err(|e| {
            let status = if matches!(
                e,
                bib_base::Error::SearchConflict(_) | bib_base::Error::MetadataConflict(_)
            ) {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(json!({"error":e.to_string()})))
        })?;
    Ok(Json(json!({"article":article})))
}

// Derived files are addressed by article+attachment rather than stored raw-file URI.
// Copy before committing ownership; source objects remain intact if validation fails.
async fn preserve_merge_assets(
    shared: &BibShared,
    input: &MergeInput,
) -> Result<(), (StatusCode, Json<Value>)> {
    if input.source_ids.is_empty()
        || input.source_ids.len() > 100
        || input.source_ids.contains(&input.target_id)
    {
        return Err(bad("Invalid merge selection"));
    }
    for id in std::iter::once(&input.target_id).chain(input.source_ids.iter()) {
        let article = shared
            .bib
            .get_article(id)
            .await
            .map_err(internal)?
            .ok_or_else(|| bad("Record unavailable"))?;
        if input.expected_revisions.get(id) != Some(&bib_base::article_revision(&article)) {
            return Err((
                StatusCode::CONFLICT,
                Json(json!({"error":"Records changed; reload duplicate review"})),
            ));
        }
    }
    let Some(storage) = shared.file_storage.as_ref() else {
        return Ok(());
    };
    for source in &input.source_ids {
        let preferred = shared
            .bib
            .zotero_attachment(source)
            .await
            .map_err(internal)?
            .map(|attachment| attachment.key);
        for attachment in shared
            .bib
            .zotero_attachments(source)
            .await
            .map_err(internal)?
        {
            let key = &attachment.key;
            let is_preferred = preferred.as_deref() == Some(key.as_str());
            let fulltext = shared
                .bib
                .get_attachment_fulltext(source, key)
                .await
                .map_err(internal)?;
            if fulltext.as_ref().is_some_and(|f| {
                f.extract_status == Some(bib_types::ExtractStatus::Running)
                    || (f.extract_status == Some(bib_types::ExtractStatus::Pending)
                        && attachment.vfs_path.is_some()
                        && attachment.link_mode != 3)
            }) {
                return Err(bad(
                    "Wait for attachment extraction to finish before merging",
                ));
            }
            let mut names = HashSet::new();
            let structure = bib_base::stored_files::structured_object_path(source, Some(key));
            let destination =
                bib_base::stored_files::structured_object_path(&input.target_id, Some(key));
            let mut structure_bytes =
                copy_optional_asset(storage, &structure, &destination).await?;
            if structure_bytes.is_none() && is_preferred {
                structure_bytes = copy_optional_asset(
                    storage,
                    &bib_base::stored_files::structured_object_path(source, None),
                    &destination,
                )
                .await?;
            }
            if let Some(bytes) = structure_bytes {
                if let Ok(cache) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(structure) = cache.get("structure") {
                        names.extend(bib_base::zotero_extract::scan_structure_image_refs(
                            structure,
                        ));
                    }
                }
            }
            for format in [
                bib_types::TextFormat::Plain,
                bib_types::TextFormat::Markdown,
            ] {
                let from = bib_base::extracted_attachment_object_path(source, key, format);
                let to = bib_base::extracted_attachment_object_path(&input.target_id, key, format);
                let mut extracted = copy_optional_asset(storage, &from, &to).await?;
                if extracted.is_none() && is_preferred {
                    extracted = copy_optional_asset(
                        storage,
                        &bib_base::extracted_object_path(source, format),
                        &to,
                    )
                    .await?;
                }
                if let Some(bytes) = extracted {
                    names.extend(bib_base::scan_image_refs(&String::from_utf8_lossy(&bytes)));
                }
            }
            if let Some(fulltext) = fulltext {
                names.extend(bib_base::scan_image_refs(
                    fulltext.text_content.as_deref().unwrap_or_default(),
                ));
            }
            for name in names {
                let from = bib_base::attachment_figure_object_path(source, key, &name);
                let to = bib_base::attachment_figure_object_path(&input.target_id, key, &name);
                if copy_optional_asset(storage, &from, &to).await?.is_none() && is_preferred {
                    copy_optional_asset(storage, &bib_base::figure_object_path(source, &name), &to)
                        .await?;
                }
            }
        }
    }
    Ok(())
}
async fn copy_optional_asset(
    storage: &vfs::OpendalFileStorage,
    from: &str,
    to: &str,
) -> Result<Option<Vec<u8>>, (StatusCode, Json<Value>)> {
    let from = bib_base::vfs_virtual_path(from).ok_or_else(|| bad("Invalid source asset path"))?;
    let to = bib_base::vfs_virtual_path(to).ok_or_else(|| bad("Invalid destination asset path"))?;
    match bib_base::read_full(storage, &from).await {
        Ok(bytes) => {
            match storage.write_bytes_if_absent(&to, bytes.clone()).await {
                Ok(()) => {}
                Err(error) if error.kind() == opendal::ErrorKind::AlreadyExists => {
                    let existing = bib_base::read_full(storage, &to).await.map_err(internal)?;
                    if existing != bytes {
                        return Err((
                            StatusCode::CONFLICT,
                            Json(
                                json!({"error":"A destination attachment asset already exists with different contents; merge cancelled"}),
                            ),
                        ));
                    }
                }
                Err(error) => return Err(internal(error)),
            }
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == opendal::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(internal(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn merge_copies_derived_assets_and_keeps_original_attachment_bytes() {
        let storage = Arc::new(vfs::OpendalFileStorage::new_temp());
        let shared = Arc::new(
            BibShared::open_in_memory()
                .await
                .unwrap()
                .with_file_storage(storage.clone()),
        );
        let target = Article::new("target", "Target");
        let source = Article::new("source", "Source");
        shared
            .bib
            .upsert_articles(&[target.clone(), source.clone()])
            .await
            .unwrap();
        let raw = "vfs:///literature/source/original.txt";
        storage
            .write_bytes(
                &bib_base::vfs_virtual_path(raw).unwrap(),
                b"original bytes".to_vec(),
            )
            .await
            .unwrap();
        let attachment = shared
            .bib
            .create_zotero_attachment(
                bib_base::NewAttachment {
                    article_id: source.id.clone(),
                    title: None,
                    filename: Some("original.txt".into()),
                    content_type: Some("text/plain".into()),
                    file_format: bib_types::FileFormat::Txt,
                    link_mode: 0,
                    path: None,
                    source_url: None,
                    vfs_path: Some(raw.into()),
                    storage_hash: None,
                    file_size: Some(14),
                },
                true,
            )
            .await
            .unwrap();
        let text_path = bib_base::extracted_attachment_object_path(
            &source.id,
            &attachment.key,
            bib_types::TextFormat::Markdown,
        );
        storage
            .write_bytes(
                &bib_base::vfs_virtual_path(&text_path).unwrap(),
                b"![figure](images/plot.png)".to_vec(),
            )
            .await
            .unwrap();
        shared
            .bib
            .record_attachment_extraction_success(
                &attachment.key,
                "![figure](images/plot.png)",
                bib_types::TextFormat::Markdown,
                "fixture",
                &text_path,
            )
            .await
            .unwrap();
        let figure =
            bib_base::attachment_figure_object_path(&source.id, &attachment.key, "plot.png");
        storage
            .write_bytes(
                &bib_base::vfs_virtual_path(&figure).unwrap(),
                b"image bytes".to_vec(),
            )
            .await
            .unwrap();
        let input = MergeInput {
            metadata: None,
            target_id: target.id.clone(),
            source_ids: vec![source.id.clone()],
            expected_revisions: HashMap::from([
                (target.id.clone(), bib_base::article_revision(&target)),
                (source.id.clone(), bib_base::article_revision(&source)),
            ]),
        };
        let _ = merge(State(shared.clone()), Json(input)).await.unwrap();
        let attachment = shared
            .bib
            .zotero_attachments(&target.id)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(attachment.vfs_path.as_deref(), Some(raw));
        assert_eq!(
            bib_base::read_full(&storage, &bib_base::vfs_virtual_path(raw).unwrap())
                .await
                .unwrap(),
            b"original bytes"
        );
        let moved =
            bib_base::attachment_figure_object_path(&target.id, &attachment.key, "plot.png");
        assert_eq!(
            bib_base::read_full(&storage, &bib_base::vfs_virtual_path(&moved).unwrap())
                .await
                .unwrap(),
            b"image bytes"
        );
        assert!(shared.bib.get_article(&source.id).await.unwrap().is_none());
    }
    #[tokio::test]
    async fn merge_asset_collision_never_overwrites_existing_destination() {
        let storage = vfs::OpendalFileStorage::new_temp();
        storage
            .write_bytes("/literature/source/asset.txt", b"source".to_vec())
            .await
            .unwrap();
        storage
            .write_bytes("/literature/target/asset.txt", b"unrelated target".to_vec())
            .await
            .unwrap();
        let error = copy_optional_asset(
            &storage,
            "vfs:///literature/source/asset.txt",
            "vfs:///literature/target/asset.txt",
        )
        .await
        .unwrap_err();
        assert_eq!(error.0, StatusCode::CONFLICT);
        assert_eq!(
            bib_base::read_full(&storage, "/literature/target/asset.txt")
                .await
                .unwrap(),
            b"unrelated target"
        );
        let (first, second) = tokio::join!(
            storage.write_bytes_if_absent("/literature/new.txt", b"first".to_vec()),
            storage.write_bytes_if_absent("/literature/new.txt", b"second".to_vec())
        );
        assert_ne!(first.is_ok(), second.is_ok());
        let actual = bib_base::read_full(&storage, "/literature/new.txt")
            .await
            .unwrap();
        assert!(actual == b"first" || actual == b"second");
    }

    #[tokio::test]
    async fn preview_and_commit_never_replace_existing_metadata() {
        let shared = Arc::new(BibShared::open_in_memory().await.unwrap());
        let mut existing = Article::new("existing", "User edited title");
        existing
            .identifiers
            .push(bib_types::Identifier::doi("10.1234/a"));
        shared.bib.upsert_article(&existing).await.unwrap();
        let contents = "@article{x,title={Imported title},doi={10.1234/a}}";
        let request = || ImportInput {
            format: "bibtex".into(),
            contents: contents.into(),
            selected_indices: Some(vec![0]),
            collection_id: None,
        };
        let Json(result) = preview(State(shared.clone()), Json(request()))
            .await
            .unwrap();
        assert_eq!(result["records"][0]["duplicate_id"], "existing");
        let Json(result) = commit(State(shared.clone()), Json(request()))
            .await
            .unwrap();
        assert_eq!(result["skipped_ids"], json!(["existing"]));
        assert_eq!(
            shared
                .bib
                .get_article("existing")
                .await
                .unwrap()
                .unwrap()
                .title,
            "User edited title"
        );
    }
}
