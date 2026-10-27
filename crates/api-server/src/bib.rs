use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::header,
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use bib_base::collections::{CollectionAssignment, CollectionChange, CollectionQuery};
use bib_base::extraction::read_full;
use bib_base::{
    AttachmentTypeSummary, BibShared, NewAttachment, attachment_figure_object_path,
    extracted_attachment_object_path, extracted_object_path, figure_object_path,
    sanitize_figure_name, scan_image_refs, spawn_attachment_extraction, stored_attachment,
    stored_fulltext, try_fetch_fulltext_with, vfs_virtual_path,
};
use bib_types::{
    AddedBy, AnnotationKind, AnnotationRect, Article, ArticleRole, ArticleSource, Author,
    Collection, CollectionStatus, ExportFormat, ExtractStatus, FileFormat, FullText,
    FullTextSource, IdKind, Identifier, StructuredSearch, TextFormat,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;
const DEFAULT_FULLTEXT_CHARS: usize = 100_000;
const MAX_FULLTEXT_CHARS: usize = 500_000;

#[derive(Deserialize, Default)]
struct FulltextPageQuery {
    offset: Option<usize>,
    limit: Option<usize>,
}

impl FulltextPageQuery {
    fn normalized(self) -> (usize, usize) {
        (
            self.offset.unwrap_or(0),
            self.limit
                .unwrap_or(DEFAULT_FULLTEXT_CHARS)
                .clamp(1, MAX_FULLTEXT_CHARS),
        )
    }
}

#[derive(Serialize)]
struct ApiError {
    error: String,
}

type ApiResult = Result<Json<Value>, (StatusCode, Json<ApiError>)>;

pub fn router(shared: BibShared) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/articles", get(list_articles).post(create_article))
        .route("/articles/unfiled", get(list_unfiled_articles))
        .route("/articles/import", post(import_article))
        .route(
            "/articles/{id}",
            get(get_article).put(update_article).delete(delete_article),
        )
        .route(
            "/articles/{id}/attachments",
            get(list_attachments).post(upload_attachment),
        )
        .route(
            "/articles/{id}/attachments/{key}",
            get(get_attachment).delete(delete_attachment_by_key),
        )
        .route(
            "/articles/{id}/attachments/{key}/preferred",
            put(set_preferred_attachment),
        )
        .route(
            "/articles/{id}/attachments/{key}/annotations",
            get(list_attachment_annotations).post(add_attachment_annotation),
        )
        .route(
            "/articles/{id}/attachments/{key}/fulltext",
            get(get_attachment_fulltext),
        )
        .route(
            "/articles/{id}/attachments/{key}/fulltext/reextract",
            post(reextract_attachment_fulltext),
        )
        .route(
            "/articles/{id}/attachments/{key}/raw",
            get(download_attachment),
        )
        .route(
            "/articles/{id}/fulltext",
            get(get_fulltext)
                .post(upload_fulltext)
                .delete(delete_fulltext),
        )
        .route(
            "/articles/{id}/fulltext/structured",
            get(get_structured_fulltext).post(generate_structured_fulltext),
        )
        .route(
            "/articles/{id}/attachments/{key}/fulltext/structured",
            get(get_attachment_structure).post(generate_attachment_structure),
        )
        .route(
            "/articles/{id}/fulltext/raw",
            get(download_fulltext).head(download_fulltext),
        )
        .route(
            "/articles/{id}/fulltext/images/{name}",
            get(download_figure),
        )
        .route(
            "/articles/{id}/fulltext/reextract",
            post(reextract_fulltext),
        )
        .route(
            "/articles/{id}/annotations",
            get(list_annotations).post(add_annotation),
        )
        .route(
            "/annotations/{id}",
            delete(delete_annotation).patch(update_annotation),
        )
        .route(
            "/collections",
            get(list_collections).post(create_collection),
        )
        .route(
            "/collections/{id}",
            get(get_collection)
                .delete(delete_collection)
                .patch(update_collection),
        )
        .route(
            "/collections/{id}/articles",
            get(list_collection_articles).post(add_to_collection),
        )
        .route(
            "/collections/{id}/articles/{article_id}",
            delete(remove_from_collection),
        )
        .route("/collections/top", get(list_top_collections))
        .route("/collections/changes", get(collection_changes))
        .route("/collections/trash", get(list_trashed_collections))
        .route("/collections/{id}/collections", get(list_child_collections))
        .route("/collections/{id}/restore", post(restore_collection))
        .route("/collections/{id}/status", put(update_collection_status))
        .route("/requests", get(list_requests))
        .route("/export", get(export))
        .route("/search/external", get(search_external))
        .with_state(Arc::new(shared))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
}

async fn health(State(shared): State<Arc<BibShared>>) -> ApiResult {
    let count = shared.bib.article_count().await.map_err(internal)?;
    Ok(Json(json!({
        "status": "ok",
        "article_count": count,
        "sources": shared.gateway.source_names(),
    })))
}

fn error(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (
        status,
        Json(ApiError {
            error: message.into(),
        }),
    )
}

fn internal(message: impl std::fmt::Display) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::INTERNAL_SERVER_ERROR, message.to_string())
}

fn parse_limit(raw: Option<&str>, default: usize) -> Result<usize, (StatusCode, Json<ApiError>)> {
    let value = raw
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default);
    if value == 0 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "limit must be greater than 0",
        ));
    }
    Ok(value.clamp(1, 500))
}

fn parse_offset(raw: Option<&str>) -> Result<usize, (StatusCode, Json<ApiError>)> {
    raw.unwrap_or("0").parse::<usize>().map_err(|_| {
        error(
            StatusCode::BAD_REQUEST,
            "offset must be a non-negative integer",
        )
    })
}

fn attachment_type_summary(summary: &AttachmentTypeSummary) -> Value {
    json!({
        "pdf": summary.pdf,
        "snapshot": summary.snapshot,
        "link": summary.link,
        "file": summary.file,
        "note": summary.note,
        "count": summary.count,
    })
}

async fn attachment_summary_map(
    shared: &BibShared,
    articles: &[Article],
) -> Result<HashMap<String, Value>, (StatusCode, Json<ApiError>)> {
    let ids = articles
        .iter()
        .map(|article| article.id.clone())
        .collect::<Vec<_>>();
    let summaries = shared
        .bib
        .attachment_type_summaries(&ids)
        .await
        .map_err(internal)?;
    Ok(summaries
        .into_iter()
        .map(|summary| {
            (
                summary.article_id.clone(),
                attachment_type_summary(&summary),
            )
        })
        .collect())
}

async fn list_articles(
    State(shared): State<Arc<BibShared>>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let query = params.get("query").map(String::as_str).unwrap_or("");
    let limit = parse_limit(params.get("limit").map(String::as_str), 100)?;
    let offset = parse_offset(params.get("offset").map(String::as_str))?;
    let page = shared
        .bib
        .query_collection_articles(CollectionQuery {
            collection_id: None,
            include_descendants: false,
            unfiled: false,
            query: query.to_owned(),
            limit,
            offset,
        })
        .await
        .map_err(collection_error)?;
    let attachment_summaries = attachment_summary_map(&shared, &page.articles).await?;
    Ok(Json(
        json!({"total":page.total,"articles":page.articles,"associations":page.associations,"attachment_summaries":attachment_summaries}),
    ))
}

async fn list_unfiled_articles(
    State(shared): State<Arc<BibShared>>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let limit = parse_limit(params.get("limit").map(String::as_str), 500)?;
    let offset = parse_offset(params.get("offset").map(String::as_str))?;
    let page = shared
        .bib
        .query_collection_articles(CollectionQuery {
            collection_id: None,
            include_descendants: false,
            unfiled: true,
            query: params.get("query").cloned().unwrap_or_default(),
            limit,
            offset,
        })
        .await
        .map_err(collection_error)?;
    let attachment_summaries = attachment_summary_map(&shared, &page.articles).await?;
    Ok(Json(
        json!({"total":page.total,"articles":page.articles,"associations":page.associations,"attachment_summaries":attachment_summaries}),
    ))
}

#[derive(Deserialize)]
struct CreateArticleRequest {
    title: String,
    doi: Option<String>,
    pmid: Option<String>,
    arxiv: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    year: Option<u16>,
    journal: Option<String>,
    volume: Option<String>,
    issue: Option<String>,
    pages: Option<String>,
    abstract_text: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    pub_types: Vec<String>,
    source: Option<String>,
}

async fn create_article(
    State(shared): State<Arc<BibShared>>,
    Json(input): Json<CreateArticleRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<ApiError>)> {
    let article = article_from_input(input)?;

    for identifier in &article.identifiers {
        if let Some(existing) = shared
            .bib
            .find_by_identifier(identifier.kind, &identifier.value)
            .await
            .map_err(internal)?
        {
            return Err(error(
                StatusCode::CONFLICT,
                format!(
                    "identifier {}:{} is already stored as {}",
                    identifier.kind.as_str(),
                    identifier.value,
                    existing.id
                ),
            ));
        }
    }

    shared
        .bib
        .upsert_article(&article)
        .await
        .map_err(internal)?;
    Ok((StatusCode::CREATED, Json(json!({ "article": article }))))
}

fn article_from_input(
    input: CreateArticleRequest,
) -> Result<Article, (StatusCode, Json<ApiError>)> {
    if input.title.trim().is_empty() {
        return Err(error(StatusCode::BAD_REQUEST, "title is required"));
    }

    let mut identifiers = Vec::new();
    if let Some(doi) = normalize_optional(input.doi.as_deref()) {
        identifiers.push(Identifier::doi(doi));
    }
    if let Some(pmid) = normalize_optional(input.pmid.as_deref()) {
        identifiers.push(Identifier::pmid(pmid));
    }
    if let Some(arxiv) = normalize_optional(input.arxiv.as_deref()) {
        identifiers.push(Identifier::new(IdKind::Arxiv, arxiv));
    }
    if identifiers.is_empty() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "at least one of doi, pmid, or arxiv is required",
        ));
    }

    let authors = input
        .authors
        .iter()
        .filter_map(|display| parse_display_author(display))
        .collect::<Vec<_>>();
    let primary = &identifiers[0];
    let now = Utc::now();

    Ok(Article {
        id: format!("{}:{}", primary.kind.as_str(), primary.value),
        title: input.title.trim().to_owned(),
        authors,
        identifiers,
        abstract_text: clean_optional(input.abstract_text),
        year: input.year,
        month: None,
        journal: clean_optional(input.journal),
        volume: clean_optional(input.volume),
        issue: clean_optional(input.issue),
        pages: clean_optional(input.pages),
        issn: None,
        essn: None,
        language: None,
        pub_types: input.pub_types,
        keywords: input.keywords,
        source: parse_article_source(input.source.as_deref()),
        created_at: Some(now),
        updated_at: Some(now),
    })
}

fn normalize_optional(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn clean_optional(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn parse_display_author(display: &str) -> Option<Author> {
    let display = display.trim();
    if display.is_empty() {
        return None;
    }
    let (last_name, fore_name) = match display.split_once(' ') {
        Some((last, fore)) => (last.to_owned(), Some(fore.trim().to_owned())),
        None => (display.to_owned(), None),
    };
    Some(Author {
        last_name,
        fore_name,
        initials: None,
        affiliation: None,
        orcid: None,
        corresponding: false,
    })
}

fn parse_article_source(source: Option<&str>) -> ArticleSource {
    match source.unwrap_or("manual").to_ascii_lowercase().as_str() {
        "pubmed" => ArticleSource::Pubmed,
        "crossref" | "doi" => ArticleSource::CrossRef,
        "arxiv" => ArticleSource::Arxiv,
        "biorxiv" => ArticleSource::Biorxiv,
        "europepmc" => ArticleSource::EuropePmc,
        "semantic_scholar" | "s2" => ArticleSource::SemanticScholar,
        "openalex" => ArticleSource::OpenAlex,
        "manual" => ArticleSource::Manual,
        _ => ArticleSource::Unknown,
    }
}

async fn import_article(
    State(shared): State<Arc<BibShared>>,
    Json(input): Json<ImportArticleRequest>,
) -> ApiResult {
    if input.id.trim().is_empty() {
        return Err(error(StatusCode::BAD_REQUEST, "id is required"));
    }
    let kind = parse_id_kind(&input.id_type).ok_or_else(|| {
        error(
            StatusCode::BAD_REQUEST,
            "id_type must be doi, pmid, arxiv, biorxiv, s2, or openalex",
        )
    })?;
    let identifier = Identifier::new(kind, input.id.trim());

    if let Some(existing) = shared
        .bib
        .find_by_identifier(identifier.kind, &identifier.value)
        .await
        .map_err(internal)?
    {
        if let Some(collection_id) = input.collection_id.as_deref() {
            shared
                .bib
                .ensure_collection_membership(collection_id, &existing.id)
                .await
                .map_err(collection_error)?;
        }
        return Ok(Json(json!({ "cached": true, "article": existing })));
    }

    let fetched = if let Some(source) = input.source.as_deref() {
        shared
            .gateway
            .fetch_from(source, &identifier)
            .await
            .ok()
            .flatten()
            .map(|article| (source.to_owned(), article))
    } else {
        shared.gateway.fetch(&identifier).await
    };

    let Some((source, mut article)) = fetched else {
        return Err(error(
            StatusCode::NOT_FOUND,
            format!("article {}:{} was not found", input.id_type, input.id),
        ));
    };
    if article.title.trim().is_empty() {
        return Err(error(
            StatusCode::BAD_GATEWAY,
            format!("source {source} returned an article without a title"),
        ));
    }
    article.updated_at = Some(Utc::now());
    save_imported_article(&shared, &article, input.collection_id.as_deref()).await?;

    let fulltext_fetched = if input.fetch_fulltext.unwrap_or(true) {
        match try_fetch_fulltext_with(&shared.europe_pmc, &article).await {
            Some(fulltext) => shared
                .bib
                .upsert_fulltext(&fulltext)
                .await
                .map_err(internal)
                .is_ok(),
            None => false,
        }
    } else {
        false
    };

    Ok(Json(json!({
        "cached": false,
        "source": source,
        "fulltext_fetched": fulltext_fetched,
        "article": article,
    })))
}

#[derive(Deserialize)]
struct ImportArticleRequest {
    collection_id: Option<String>,
    id_type: String,
    id: String,
    source: Option<String>,
    fetch_fulltext: Option<bool>,
}

async fn save_imported_article(
    shared: &BibShared,
    article: &Article,
    collection_id: Option<&str>,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    if let Some(id) = collection_id {
        shared
            .bib
            .upsert_articles_with_collection(
                std::slice::from_ref(article),
                id,
                &[CollectionAssignment {
                    article_id: article.id.clone(),
                    role: ArticleRole::Referenced,
                    added_by: AddedBy::User,
                    note: None,
                }],
            )
            .await
            .map_err(collection_error)?;
    } else {
        shared.bib.upsert_article(article).await.map_err(internal)?;
    }
    Ok(())
}

fn parse_id_kind(value: &str) -> Option<IdKind> {
    match value.to_ascii_lowercase().as_str() {
        "doi" => Some(IdKind::Doi),
        "pmid" => Some(IdKind::Pmid),
        "pmc" => Some(IdKind::Pmc),
        "arxiv" => Some(IdKind::Arxiv),
        "biorxiv" => Some(IdKind::Biorxiv),
        "s2" => Some(IdKind::S2),
        "openalex" => Some(IdKind::OpenAlex),
        _ => None,
    }
}

async fn get_article(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Query(page_query): Query<FulltextPageQuery>,
) -> ApiResult {
    let article = shared
        .bib
        .get_article(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("article {id} not found")))?;
    let (offset, limit) = page_query.normalized();
    let fulltext_page = shared
        .bib
        .get_fulltext_page(&id, offset, limit)
        .await
        .map_err(internal)?;
    let annotations = shared
        .bib
        .list_annotations(&id, None)
        .await
        .map_err(internal)?;
    let pagination = fulltext_page.as_ref().map(|page| {
        json!({
            "offset": page.offset,
            "limit": page.limit,
            "total_chars": page.total_chars,
            "truncated": page.truncated,
            "next_offset": page.next_offset,
        })
    });
    let fulltext = fulltext_page.map(|page| page.fulltext);

    Ok(Json(json!({
        "article": article,
        "fulltext": fulltext,
        "fulltext_pagination": pagination,
        "annotations": annotations,
    })))
}

async fn update_article(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(mut article): Json<Article>,
) -> ApiResult {
    let existing = shared
        .bib
        .get_article(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("article {id} not found")))?;

    if article.title.trim().is_empty() {
        return Err(error(StatusCode::BAD_REQUEST, "title cannot be empty"));
    }
    article.id = id;
    article.created_at = existing.created_at;
    article.updated_at = Some(Utc::now());
    shared
        .bib
        .upsert_article(&article)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "article": article })))
}

async fn delete_article(State(shared): State<Arc<BibShared>>, Path(id): Path<String>) -> ApiResult {
    let fulltext = shared.bib.get_fulltext(&id).await.map_err(internal)?;
    let removed = shared.bib.delete_article(&id).await.map_err(internal)?;
    if removed > 0 {
        if let Some(fulltext) = fulltext {
            if let Some(path) = vfs_virtual_path(&fulltext.file_path) {
                if let Err(cleanup_error) = delete_stored_file(&shared, &path).await {
                    tracing::warn!(
                        path = %path,
                        error = %cleanup_error.1.error,
                        "article was removed but its full-text original file remains"
                    );
                }
            }
            delete_article_figures(&shared, &fulltext).await;
        }
    }
    Ok(Json(
        json!({ "deleted": removed > 0, "id": id, "rows_removed": removed }),
    ))
}

async fn structure_response(
    shared: &BibShared,
    id: &str,
    key: Option<&str>,
    generate: bool,
) -> ApiResult {
    let fulltext = match key {
        Some(key) => shared.bib.get_attachment_fulltext(id, key).await,
        None => shared.bib.get_fulltext(id).await,
    }
    .map_err(internal)?
    .ok_or_else(|| error(StatusCode::NOT_FOUND, "PDF attachment was not found"))?;
    if fulltext.file_format != FileFormat::Pdf {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Reading Mode requires a PDF attachment",
        ));
    }
    file_storage(shared)?;
    let structure = bib_base::zotero_extract::structured_fulltext(shared, id, key, generate)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                "Zotero structured text has not been generated yet",
            )
        })?;
    Ok(Json(json!({ "structure": structure })))
}

async fn get_structured_fulltext(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    structure_response(&shared, &id, None, false).await
}

async fn generate_structured_fulltext(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    structure_response(&shared, &id, None, true).await
}

async fn get_attachment_structure(
    State(shared): State<Arc<BibShared>>,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult {
    structure_response(&shared, &id, Some(&key), false).await
}

async fn generate_attachment_structure(
    State(shared): State<Arc<BibShared>>,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult {
    structure_response(&shared, &id, Some(&key), true).await
}

async fn get_fulltext(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Query(page_query): Query<FulltextPageQuery>,
) -> ApiResult {
    let (offset, limit) = page_query.normalized();
    let page = shared
        .bib
        .get_fulltext_page(&id, offset, limit)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                format!("full text for {id} not found"),
            )
        })?;
    let pagination = json!({
        "offset": page.offset,
        "limit": page.limit,
        "total_chars": page.total_chars,
        "truncated": page.truncated,
        "next_offset": page.next_offset,
    });
    Ok(Json(
        json!({ "fulltext": page.fulltext, "pagination": pagination }),
    ))
}

async fn list_attachments(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    let attachments = shared.bib.zotero_attachments(&id).await.map_err(internal)?;
    Ok(Json(json!({ "attachments": attachments })))
}

#[derive(Default)]
struct AttachmentUpload {
    filename: Option<String>,
    content_type: Option<String>,
    title: Option<String>,
    preferred: Option<bool>,
    content: Option<axum::body::Bytes>,
}

async fn upload_attachment(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<ApiError>)> {
    let article = shared
        .bib
        .get_article(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("article {id} not found")))?;
    let mut upload = AttachmentUpload::default();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|multipart_error| error(StatusCode::BAD_REQUEST, multipart_error.to_string()))?
    {
        match field.name() {
            Some("file") => {
                upload.filename = field
                    .file_name()
                    .map(str::to_owned)
                    .or_else(|| field.content_type().map(str::to_owned));
                upload.content_type = field.content_type().map(str::to_owned);
                let bytes = field.bytes().await.map_err(|multipart_error| {
                    error(StatusCode::BAD_REQUEST, multipart_error.to_string())
                })?;
                if bytes.len() > MAX_UPLOAD_BYTES {
                    return Err(error(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        format!("upload exceeds {} byte limit", MAX_UPLOAD_BYTES),
                    ));
                }
                upload.content = Some(bytes);
            }
            Some("title") => {
                upload.title = Some(
                    field
                        .text()
                        .await
                        .map_err(|multipart_error| {
                            error(StatusCode::BAD_REQUEST, multipart_error.to_string())
                        })?
                        .trim()
                        .to_owned(),
                )
                .filter(|title| !title.is_empty());
            }
            Some("preferred") => {
                let value = field.text().await.map_err(|multipart_error| {
                    error(StatusCode::BAD_REQUEST, multipart_error.to_string())
                })?;
                upload.preferred = Some(matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                ));
            }
            _ => {}
        }
    }

    let Some(content) = upload.content else {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "multipart field 'file' is required",
        ));
    };
    let filename = upload
        .filename
        .unwrap_or_else(|| "attachment.txt".to_owned());
    let extension = filename
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .unwrap_or("txt");
    let format = FileFormat::from_extension(extension);
    let content_type = upload.content_type.or_else(|| {
        Some(
            match format {
                FileFormat::Pdf => "application/pdf",
                FileFormat::Html => "text/html",
                FileFormat::Txt => "text/plain",
            }
            .to_owned(),
        )
    });
    let make_preferred = upload.preferred.unwrap_or(true);
    let previous_preferred = shared.bib.zotero_attachment(&id).await.map_err(internal)?;

    let input = NewAttachment {
        article_id: article.id.clone(),
        title: upload.title,
        filename: Some(filename.clone()),
        content_type,
        file_format: format,
        link_mode: bib_base::LINK_MODE_IMPORTED_FILE,
        path: None,
        source_url: None,
        vfs_path: None,
        storage_hash: None,
        file_size: Some(content.len() as i64),
    };
    let attachment = shared
        .bib
        .create_zotero_attachment(input, make_preferred)
        .await
        .map_err(internal)?;
    let stored = stored_attachment(&article.id, &attachment.key, &filename, &content);
    let virtual_path = vfs_virtual_path(&stored.path).expect("stored attachment uses a VFS path");
    if let Err(write_error) = write_stored_file(&shared, &virtual_path, content.to_vec()).await {
        let _ = shared
            .bib
            .delete_zotero_attachment(&id, &attachment.key)
            .await;
        return Err(write_error);
    }
    let attachment = match shared
        .bib
        .set_zotero_attachment_storage(
            &id,
            &attachment.key,
            &stored.path,
            &stored.file_hash,
            content.len() as i64,
        )
        .await
    {
        Ok(Some(attachment)) => attachment,
        Ok(None) => {
            let _ = shared
                .bib
                .delete_zotero_attachment(&id, &attachment.key)
                .await;
            let _ = delete_stored_file(&shared, &virtual_path).await;
            return Err(error(
                StatusCode::NOT_FOUND,
                "attachment disappeared while storing its object",
            ));
        }
        Err(db_error) => {
            let _ = shared
                .bib
                .delete_zotero_attachment(&id, &attachment.key)
                .await;
            let _ = delete_stored_file(&shared, &virtual_path).await;
            return Err(internal(db_error));
        }
    };

    if make_preferred {
        let fulltext = FullText {
            article_id: article.id.clone(),
            attachment_key: Some(attachment.key.clone()),
            file_path: stored.path.clone(),
            file_format: format,
            text_content: None,
            source: FullTextSource::UserUpload,
            file_hash: Some(stored.file_hash.clone()),
            file_size: Some(content.len() as i64),
            uploaded_at: Some(Utc::now()),
            extract_status: Some(ExtractStatus::Pending),
            text_format: None,
            extracted_by: None,
            extract_error: None,
        };
        if let Err(db_error) = shared.bib.upsert_fulltext_pending(&fulltext).await {
            let _ = shared
                .bib
                .delete_zotero_attachment(&id, &attachment.key)
                .await;
            if let Some(previous) = previous_preferred {
                let _ = shared
                    .bib
                    .set_preferred_zotero_attachment(&id, &previous.key)
                    .await;
            }
            let _ = delete_stored_file(&shared, &virtual_path).await;
            return Err(internal(db_error));
        }
        if let Some(previous) = shared.bib.get_fulltext(&id).await.map_err(internal)? {
            delete_article_figures(&shared, &previous).await;
        }
    }
    spawn_attachment_extraction(
        shared.as_ref().clone(),
        article.id.clone(),
        attachment.key.clone(),
    );

    Ok((
        StatusCode::CREATED,
        Json(json!({ "attachment": attachment })),
    ))
}

async fn get_attachment(
    State(shared): State<Arc<BibShared>>,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult {
    let attachment = shared
        .bib
        .zotero_attachment_by_key(&id, &key)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("attachment {key} not found")))?;
    Ok(Json(json!({ "attachment": attachment })))
}

async fn download_attachment(
    State(shared): State<Arc<BibShared>>,
    Path((id, key)): Path<(String, String)>,
) -> Result<Response, (StatusCode, Json<ApiError>)> {
    let attachment = shared
        .bib
        .zotero_attachment_by_key(&id, &key)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("attachment {key} not found")))?;
    let virtual_path = vfs_virtual_path(
        attachment
            .vfs_path
            .as_deref()
            .ok_or_else(|| error(StatusCode::NOT_FOUND, "attachment has no VFS object"))?,
    )
    .ok_or_else(|| error(StatusCode::NOT_FOUND, "invalid attachment VFS URI"))?;
    let bytes = read_full(file_storage(&shared)?, &virtual_path)
        .await
        .map_err(storage_error)?;
    let content_type = attachment.content_type.clone().unwrap_or_else(|| {
        match attachment
            .original_filename
            .as_deref()
            .and_then(|filename| filename.rsplit_once('.'))
            .map(|(_, extension)| extension)
            .unwrap_or("txt")
        {
            "pdf" => "application/pdf",
            "html" | "htm" => "text/html",
            _ => "text/plain",
        }
        .to_owned()
    });
    let filename = attachment.original_filename.clone().unwrap_or(key);
    let mut response = Response::new(axum::body::Body::from(bytes.to_vec()));
    let response_headers = response.headers_mut();
    response_headers.insert(
        header::CONTENT_TYPE,
        content_type.parse().expect("valid content type"),
    );
    response_headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{filename}\"; filename*=UTF-8''{filename}")
            .parse()
            .expect("valid header"),
    );
    response_headers.insert(
        header::CONTENT_LENGTH,
        bytes.len().to_string().parse().expect("valid length"),
    );
    response_headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        "nosniff".parse().expect("valid header"),
    );
    response_headers.insert(
        header::CACHE_CONTROL,
        "private, no-store".parse().expect("valid header"),
    );
    Ok(response)
}

async fn set_preferred_attachment(
    State(shared): State<Arc<BibShared>>,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult {
    let attachment = shared
        .bib
        .zotero_attachment_by_key(&id, &key)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("attachment {key} not found")))?;
    let virtual_path = vfs_virtual_path(
        attachment
            .vfs_path
            .as_deref()
            .ok_or_else(|| error(StatusCode::BAD_REQUEST, "attachment has no VFS object"))?,
    )
    .ok_or_else(|| error(StatusCode::BAD_REQUEST, "invalid attachment VFS URI"))?;
    let _ = virtual_path;
    let format = FileFormat::from_extension(
        attachment
            .original_filename
            .as_deref()
            .and_then(|filename| filename.rsplit_once('.'))
            .map(|(_, extension)| extension)
            .unwrap_or("txt"),
    );
    if !shared
        .bib
        .set_preferred_zotero_attachment(&id, &key)
        .await
        .map_err(internal)?
    {
        return Err(error(
            StatusCode::NOT_FOUND,
            format!("attachment {key} not found"),
        ));
    }
    let fulltext = FullText {
        article_id: id.clone(),
        attachment_key: Some(key.clone()),
        file_path: attachment.vfs_path.clone().expect("checked above"),
        file_format: format,
        text_content: None,
        source: FullTextSource::UserUpload,
        file_hash: attachment.storage_hash.clone(),
        file_size: attachment.file_size,
        uploaded_at: Some(Utc::now()),
        extract_status: Some(ExtractStatus::Pending),
        text_format: None,
        extracted_by: None,
        extract_error: None,
    };
    shared
        .bib
        .upsert_fulltext_pending(&fulltext)
        .await
        .map_err(internal)?;
    if let Some(previous) = shared.bib.get_fulltext(&id).await.map_err(internal)? {
        delete_article_figures(&shared, &previous).await;
    }
    spawn_attachment_extraction(shared.as_ref().clone(), id.clone(), key.clone());
    Ok(Json(json!({ "attachment": attachment })))
}

async fn delete_attachment_by_key(
    State(shared): State<Arc<BibShared>>,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult {
    let existing_fulltext = shared
        .bib
        .get_attachment_fulltext(&id, &key)
        .await
        .map_err(internal)?;
    let deleted = shared
        .bib
        .delete_zotero_attachment(&id, &key)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("attachment {key} not found")))?;

    if deleted.was_preferred {
        let legacy = shared.bib.get_fulltext(&id).await.map_err(internal)?;
        shared.bib.delete_fulltext(&id).await.map_err(internal)?;
        if let Some(legacy) = legacy {
            delete_article_figures(&shared, &legacy).await;
        }
    }
    if let Some(existing) = existing_fulltext {
        delete_attachment_figures(&shared, &id, &key, &existing).await;
    }
    if let Some(path) = deleted
        .attachment
        .vfs_path
        .as_deref()
        .and_then(vfs_virtual_path)
    {
        if let Err(cleanup_error) = delete_stored_file(&shared, &path).await {
            tracing::warn!(
                path = %path,
                error = %cleanup_error.1.error,
                "attachment database row was removed but its object remains"
            );
        }
    }
    Ok(Json(json!({
        "deleted": true,
        "attachment": deleted.attachment,
        "was_preferred": deleted.was_preferred,
    })))
}

async fn get_attachment_fulltext(
    State(shared): State<Arc<BibShared>>,
    Path((id, key)): Path<(String, String)>,
    Query(page_query): Query<FulltextPageQuery>,
) -> ApiResult {
    let (offset, limit) = page_query.normalized();
    let fulltext = shared
        .bib
        .get_attachment_fulltext(&id, &key)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("attachment {key} not found")))?;
    let total_chars = fulltext
        .text_content
        .as_deref()
        .map_or(0, |content| content.chars().count());
    let text = fulltext
        .text_content
        .as_deref()
        .map(|content| content.chars().skip(offset).take(limit).collect::<String>());
    let returned = text.as_deref().map_or(0, |content| content.chars().count());
    let next_offset = (offset + returned < total_chars).then_some(offset + returned);
    let mut fulltext = fulltext;
    fulltext.text_content = text;
    let pagination = json!({
        "offset": offset,
        "limit": limit,
        "total_chars": total_chars,
        "truncated": next_offset.is_some(),
        "next_offset": next_offset,
    });
    Ok(Json(
        json!({ "fulltext": fulltext, "pagination": pagination }),
    ))
}

async fn reextract_attachment_fulltext(
    State(shared): State<Arc<BibShared>>,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult {
    let attachment = shared
        .bib
        .zotero_attachment_by_key(&id, &key)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("attachment {key} not found")))?;
    if vfs_virtual_path(attachment.vfs_path.as_deref().unwrap_or_default()).is_none() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            format!("attachment {key} has no VFS-stored original"),
        ));
    }
    let existing = shared
        .bib
        .get_attachment_fulltext(&id, &key)
        .await
        .map_err(internal)?;
    if let Some(existing) = existing {
        delete_attachment_figures(&shared, &id, &key, &existing).await;
    }
    shared
        .bib
        .restart_attachment_extraction(&key)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                format!("full text for {key} not found"),
            )
        })?;
    spawn_attachment_extraction(shared.as_ref().clone(), id.clone(), key.clone());
    let fulltext = shared
        .bib
        .get_attachment_fulltext(&id, &key)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "fulltext": fulltext })))
}

async fn upload_fulltext(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> ApiResult {
    let article = shared
        .bib
        .get_article(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("article {id} not found")))?;

    let mut filename = None;
    let mut content = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|multipart_error| error(StatusCode::BAD_REQUEST, multipart_error.to_string()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        filename = field
            .file_name()
            .map(str::to_owned)
            .or_else(|| field.content_type().map(str::to_owned));
        let bytes = field.bytes().await.map_err(|multipart_error| {
            error(StatusCode::BAD_REQUEST, multipart_error.to_string())
        })?;
        if bytes.len() > MAX_UPLOAD_BYTES {
            return Err(error(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("upload exceeds {} byte limit", MAX_UPLOAD_BYTES),
            ));
        }
        content = Some(bytes);
        break;
    }

    let Some(content) = content else {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "multipart field 'file' is required",
        ));
    };
    let filename = filename.unwrap_or_else(|| "upload.txt".to_owned());
    let extension = filename
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .unwrap_or("txt");
    let format = FileFormat::from_extension(extension);

    let stored = stored_fulltext(&article.id, &filename, &content);
    let path = vfs_virtual_path(&stored.path).expect("stored fulltext uses a VFS path");
    write_stored_file(&shared, &path, content.to_vec()).await?;
    let fulltext = FullText {
        article_id: article.id.clone(),
        attachment_key: None,
        file_path: stored.path,
        file_format: format,
        text_content: None,
        source: FullTextSource::UserUpload,
        file_hash: Some(stored.file_hash),
        file_size: Some(content.len() as i64),
        uploaded_at: Some(Utc::now()),
        // Text extraction happens in the background (spawn_extraction
        // below); the upload response returns immediately with a pending
        // status and clients poll until done/failed.
        extract_status: Some(ExtractStatus::Pending),
        text_format: None,
        extracted_by: None,
        extract_error: None,
    };
    let previous = shared
        .bib
        .get_fulltext(&article.id)
        .await
        .map_err(internal)?;
    if let Err(db_error) = shared.bib.upsert_fulltext_pending(&fulltext).await {
        if previous.as_ref().map(|old| old.file_path.clone()) != Some(fulltext.file_path.clone()) {
            if let Err(cleanup_error) = delete_stored_file(&shared, &path).await {
                tracing::warn!(
                    path = %path,
                    error = %cleanup_error.1.error,
                    "failed to remove new full-text object after database rejection"
                );
            }
        }
        return Err(internal(db_error));
    }
    if let Some(previous) = previous {
        if previous.file_path != fulltext.file_path {
            if let Some(path) = vfs_virtual_path(&previous.file_path) {
                if let Err(cleanup_error) = delete_stored_file(&shared, &path).await {
                    tracing::warn!(
                        path = %path,
                        error = %cleanup_error.1.error,
                        "old full-text object remains after successful database update"
                    );
                }
            }
        }
        // The row just reset to pending, so the old markdown (and any
        // figures only it referenced) is gone regardless of whether the
        // re-extraction succeeds.
        delete_article_figures(&shared, &previous).await;
    }
    // Content-addressed paths guarantee the queued task reads the object
    // just written, even though this handler returns immediately.
    if let Some(preferred) = shared
        .bib
        .zotero_attachment(&article.id)
        .await
        .map_err(internal)?
    {
        spawn_attachment_extraction(shared.as_ref().clone(), article.id.clone(), preferred.key);
    }
    let fulltext = shared
        .bib
        .get_fulltext(&article.id)
        .await
        .map_err(internal)?
        .unwrap_or(fulltext);
    Ok(Json(json!({ "fulltext": fulltext })))
}

/// Queue a re-extraction of an already-stored full text.
///
/// Only VFS-stored originals can be re-read; inline-text sources (e.g.
/// Europe PMC paths) must be re-uploaded instead. The row resets to
/// `pending`, a background task claims it, and the response carries the
/// reset row so clients can poll for the terminal state.
async fn reextract_fulltext(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    let existing = shared
        .bib
        .get_fulltext(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                format!("full text for {id} not found"),
            )
        })?;
    if vfs_virtual_path(&existing.file_path).is_none() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            format!(
                "original file is not VFS-stored ({}); re-upload the file to re-extract",
                existing.file_path
            ),
        ));
    }
    // The restart below clears the old markdown from the row, so this is
    // the last point where the figures it referenced are identifiable.
    let preferred = shared.bib.zotero_attachment(&id).await.map_err(internal)?;
    let Some(preferred) = preferred else {
        return Err(error(
            StatusCode::NOT_FOUND,
            format!("full text for {id} not found"),
        ));
    };
    delete_attachment_figures(&shared, &id, &preferred.key, &existing).await;
    if shared
        .bib
        .restart_attachment_extraction(&preferred.key)
        .await
        .map_err(internal)?
        .is_none()
    {
        return Err(error(
            StatusCode::NOT_FOUND,
            format!("full text for {id} not found"),
        ));
    }
    spawn_attachment_extraction(shared.as_ref().clone(), id.clone(), preferred.key.clone());
    let fulltext = shared.bib.get_fulltext(&id).await.map_err(internal)?;
    Ok(Json(json!({ "fulltext": fulltext })))
}

async fn download_fulltext(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, Json<ApiError>)> {
    let fulltext = shared
        .bib
        .get_fulltext(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                format!("full text for {id} not found"),
            )
        })?;
    let path = vfs_virtual_path(&fulltext.file_path).ok_or_else(|| {
        error(
            StatusCode::NOT_FOUND,
            "original file is not stored in the VFS",
        )
    })?;
    let size = shared
        .file_storage
        .as_ref()
        .ok_or_else(|| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "bibliography VFS storage is not configured",
            )
        })?
        .content_length(&path)
        .await
        .map_err(storage_error)?;

    let etag = fulltext
        .file_hash
        .as_deref()
        .map(|hash| format!("\"{hash}\""));
    let if_range_matches = headers
        .get(header::IF_RANGE)
        .is_none_or(|value| etag.as_deref().is_some_and(|etag| value == etag));
    let requested = if if_range_matches {
        parse_range_header(&headers, size)?
    } else {
        None
    };
    let partial = requested.is_some();
    let (status, start, end) = match requested {
        Some((start, end)) => (StatusCode::PARTIAL_CONTENT, start, end),
        None => (StatusCode::OK, 0, size),
    };

    let filename = path.rsplit('/').next().unwrap_or("fulltext");
    let content_type = match fulltext.file_format {
        FileFormat::Pdf => "application/pdf",
        FileFormat::Html => "application/octet-stream",
        FileFormat::Txt => "text/plain; charset=utf-8",
    };
    let body = if method == Method::HEAD {
        axum::body::Body::empty()
    } else {
        let storage = shared.file_storage.clone().ok_or_else(|| {
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "bibliography VFS storage is not configured",
            )
        })?;
        let storage_end = if partial { end + 1 } else { end };
        let stream = storage
            .read_stream(&path, start..storage_end)
            .await
            .map_err(storage_error)?;
        axum::body::Body::from_stream(stream)
    };
    let mut response = Response::new(body);
    let response_headers = response.headers_mut();
    response_headers.insert(
        header::CONTENT_TYPE,
        content_type.parse().expect("valid content type"),
    );
    response_headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{filename}\"; filename*=UTF-8''{filename}")
            .parse()
            .expect("valid filename"),
    );
    response_headers.insert(
        header::ACCEPT_RANGES,
        "bytes".parse().expect("static header"),
    );
    response_headers.insert(
        header::CONTENT_LENGTH,
        (end - start).to_string().parse().expect("valid length"),
    );
    if let Some(etag) = etag {
        response_headers.insert(header::ETAG, etag.parse().expect("quoted hash"));
    }
    if status == StatusCode::PARTIAL_CONTENT {
        response_headers.insert(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{size}")
                .parse()
                .expect("valid content range"),
        );
    }
    response_headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        "nosniff".parse().expect("static header"),
    );
    response_headers.insert(
        header::CACHE_CONTROL,
        "private, no-store".parse().expect("static header"),
    );
    *response.status_mut() = status;
    Ok(response)
}

/// Serve one figure image extracted alongside the markdown full text.
///
/// Figures are small (bounded at extraction time), so unlike the raw-file
/// route there is no Range support — the whole object streams inline for
/// `<img>` rendering.
async fn download_figure(
    State(shared): State<Arc<BibShared>>,
    Path((id, name)): Path<(String, String)>,
) -> Result<Response, (StatusCode, Json<ApiError>)> {
    let fulltext = shared.bib.get_fulltext(&id).await.map_err(internal)?;
    if fulltext.is_none() {
        return Err(error(
            StatusCode::NOT_FOUND,
            format!("full text for {id} not found"),
        ));
    }
    let name = sanitize_figure_name(&name).ok_or_else(|| {
        error(
            StatusCode::BAD_REQUEST,
            "figure name must be a plain file name ([A-Za-z0-9._-], no leading dot)",
        )
    })?;
    let content_type = match name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => {
            return Err(error(
                StatusCode::NOT_FOUND,
                format!("figure {name} has an unsupported file type"),
            ));
        }
    };
    let storage = file_storage(&shared)?;
    let object = match fulltext
        .as_ref()
        .and_then(|fulltext| fulltext.attachment_key.as_deref())
    {
        Some(key) => attachment_figure_object_path(&id, key, &name),
        None => figure_object_path(&id, &name),
    };
    let path = vfs_virtual_path(&object).expect("figure path is VFS-addressable");
    let length = storage.content_length(&path).await.map_err(|failure| {
        if failure.kind() == opendal::ErrorKind::NotFound {
            error(
                StatusCode::NOT_FOUND,
                format!("figure {name} not found for this article"),
            )
        } else {
            storage_error(failure)
        }
    })?;
    // read_range's read lock releases when the call returns, unlike
    // read_stream whose guard lives as long as the response body — figures
    // are capped at extraction time, so buffering them avoids an abandoned
    // download pinning the object against later writers.
    let bytes = storage
        .read_range(&path, 0..length)
        .await
        .map_err(storage_error)?;
    let mut response = Response::new(axum::body::Body::from(bytes.to_vec()));
    let response_headers = response.headers_mut();
    response_headers.insert(
        header::CONTENT_TYPE,
        content_type.parse().expect("valid content type"),
    );
    response_headers.insert(
        header::CONTENT_DISPOSITION,
        format!("inline; filename=\"{name}\"")
            .parse()
            .expect("valid filename"),
    );
    response_headers.insert(
        header::CONTENT_LENGTH,
        length.to_string().parse().expect("valid length"),
    );
    response_headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        "nosniff".parse().expect("static header"),
    );
    response_headers.insert(
        header::CACHE_CONTROL,
        "private, no-store".parse().expect("static header"),
    );
    Ok(response)
}

async fn delete_fulltext(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    let existing = shared.bib.get_fulltext(&id).await.map_err(internal)?;
    shared.bib.delete_fulltext(&id).await.map_err(internal)?;
    if let Some(existing) = existing {
        if let Some(path) = vfs_virtual_path(&existing.file_path) {
            if let Err(cleanup_error) = delete_stored_file(&shared, &path).await {
                tracing::warn!(
                    path = %path,
                    error = %cleanup_error.1.error,
                    "full-text database row was removed but its original file remains"
                );
            }
        }
        delete_article_figures(&shared, &existing).await;
    }
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn list_annotations(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    let annotations = shared
        .bib
        .list_annotations(&id, None)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "annotations": annotations })))
}

#[derive(Deserialize)]
struct AddAnnotationRequest {
    content: String,
    kind: Option<String>,
    page: Option<u32>,
    color: Option<String>,
    rects: Option<Vec<AnnotationRect>>,
    attachment_key: Option<String>,
}

async fn add_annotation(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<AddAnnotationRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<ApiError>)> {
    if input.content.trim().is_empty() {
        return Err(error(StatusCode::BAD_REQUEST, "content is required"));
    }
    let kind = match input.kind.as_deref().map(str::to_ascii_lowercase) {
        Some(ref kind) if kind == "highlight" => AnnotationKind::Highlight,
        Some(ref kind) if kind == "comment" => AnnotationKind::Comment,
        _ => AnnotationKind::Note,
    };
    let annotation = shared
        .bib
        .add_annotation(
            &id,
            kind,
            input.content.trim(),
            input.page,
            input.color,
            input.rects,
            input.attachment_key.as_deref(),
        )
        .await
        .map_err(internal)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "annotation": annotation })),
    ))
}

async fn update_annotation(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(patch): Json<bib_types::AnnotationUpdate>,
) -> ApiResult {
    if patch.comment.is_none() && patch.tags.is_none() && patch.color.is_none() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "No annotation changes were provided",
        ));
    }
    if patch
        .comment
        .as_ref()
        .is_some_and(|comment| comment.chars().count() > 20_000)
        || patch.tags.as_ref().is_some_and(|tags| {
            tags.len() > 50
                || tags
                    .iter()
                    .any(|tag| tag.trim().is_empty() || tag.chars().count() > 80)
        })
    {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Annotation comment or tags exceed their limits",
        ));
    }
    if let Some(color) = patch.color.as_deref() {
        let hex = color.strip_prefix('#').is_some_and(|hex| {
            matches!(hex.len(), 3 | 6 | 8) && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
        if !hex
            && ![
                "yellow", "red", "green", "blue", "purple", "magenta", "orange", "gray",
            ]
            .contains(&color)
        {
            return Err(error(StatusCode::BAD_REQUEST, "Invalid annotation color"));
        }
    }
    let annotation = shared
        .bib
        .update_annotation(&id, patch)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "Annotation not found"))?;
    Ok(Json(json!({ "annotation": annotation })))
}

async fn delete_annotation(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    shared.bib.delete_annotation(&id).await.map_err(internal)?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn list_attachment_annotations(
    State(shared): State<Arc<BibShared>>,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult {
    shared
        .bib
        .zotero_attachment_by_key(&id, &key)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("attachment {key} not found")))?;
    let annotations = shared
        .bib
        .list_annotations(&id, Some(&key))
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "annotations": annotations })))
}

async fn add_attachment_annotation(
    State(shared): State<Arc<BibShared>>,
    Path((id, key)): Path<(String, String)>,
    Json(input): Json<AddAnnotationRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<ApiError>)> {
    shared
        .bib
        .zotero_attachment_by_key(&id, &key)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("attachment {key} not found")))?;
    if input.content.trim().is_empty() {
        return Err(error(StatusCode::BAD_REQUEST, "content is required"));
    }
    let kind = match input.kind.as_deref().map(str::to_ascii_lowercase) {
        Some(ref kind) if kind == "highlight" => AnnotationKind::Highlight,
        Some(ref kind) if kind == "comment" => AnnotationKind::Comment,
        _ => AnnotationKind::Note,
    };
    let annotation = shared
        .bib
        .add_annotation(
            &id,
            kind,
            input.content.trim(),
            input.page,
            input.color,
            input.rects,
            Some(&key),
        )
        .await
        .map_err(internal)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "annotation": annotation })),
    ))
}

fn collection_error(e: bib_base::error::Error) -> (StatusCode, Json<ApiError>) {
    use bib_base::error::Error;
    let status = match &e {
        Error::NotFound(_) => StatusCode::NOT_FOUND,
        Error::InvalidCollection(_) => StatusCode::BAD_REQUEST,
        Error::CollectionConflict(_) => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, e.to_string())
}

fn bool_param(
    params: &HashMap<String, String>,
    key: &str,
) -> Result<bool, (StatusCode, Json<ApiError>)> {
    match params.get(key).map(String::as_str) {
        None | Some("false") | Some("0") => Ok(false),
        Some("true") | Some("1") => Ok(true),
        _ => Err(error(
            StatusCode::BAD_REQUEST,
            format!("{key} must be a boolean"),
        )),
    }
}

async fn list_collections(
    State(shared): State<Arc<BibShared>>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let include_deleted = bool_param(&params, "include_deleted")?;
    if params.get("view").is_some_and(|s| s == "summary") {
        let collections = shared
            .bib
            .list_collection_summaries(include_deleted)
            .await
            .map_err(collection_error)?;
        return Ok(Json(json!({"collections":collections})));
    }
    let collections = shared
        .bib
        .list_collections_including_deleted(None, include_deleted)
        .await
        .map_err(collection_error)?;
    Ok(Json(json!({"collections":collections})))
}

async fn collection_changes(
    State(shared): State<Arc<BibShared>>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let mut receiver = shared.bib.subscribe_collection_changes();
    let since = params
        .get("since")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| error(StatusCode::BAD_REQUEST, "since must be a revision number"))
        })
        .transpose()?;
    if since == Some(*receiver.borrow_and_update()) {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(25), receiver.changed()).await;
    }
    let revision = *receiver.borrow_and_update();
    Ok(Json(json!({"revision":revision})))
}

async fn list_top_collections(State(shared): State<Arc<BibShared>>) -> ApiResult {
    let collections = shared
        .bib
        .list_collections(None)
        .await
        .map_err(collection_error)?;
    Ok(Json(
        json!({"collections":collections.into_iter().filter(|c|c.parent_id.is_none()).collect::<Vec<_>>()}),
    ))
}
async fn list_child_collections(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    shared
        .bib
        .get_collection(&id)
        .await
        .map_err(collection_error)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "collection not found"))?;
    let collections = shared
        .bib
        .list_collections(None)
        .await
        .map_err(collection_error)?;
    Ok(Json(
        json!({"collections":collections.into_iter().filter(|c|c.parent_id.as_deref()==Some(id.as_str())).collect::<Vec<_>>()}),
    ))
}
async fn list_trashed_collections(State(shared): State<Arc<BibShared>>) -> ApiResult {
    let collections = shared
        .bib
        .list_collections_including_deleted(None, true)
        .await
        .map_err(collection_error)?;
    Ok(Json(
        json!({"collections":collections.into_iter().filter(|c|c.deleted).collect::<Vec<_>>()}),
    ))
}
async fn update_collection(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<Value>,
) -> ApiResult {
    let object = input
        .as_object()
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "expected an object"))?;
    let name = object
        .get("name")
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| error(StatusCode::BAD_REQUEST, "name must be a string"))
        })
        .transpose()?;
    let parent_id = object
        .get("parent_id")
        .map(|v| {
            if v.is_null() {
                Ok(None)
            } else {
                v.as_str().map(|s| Some(s.to_owned())).ok_or_else(|| {
                    error(
                        StatusCode::BAD_REQUEST,
                        "parent_id must be a string or null",
                    )
                })
            }
        })
        .transpose()?;
    shared
        .bib
        .change_collection(&id, CollectionChange::Update { name, parent_id })
        .await
        .map_err(collection_error)?;
    get_collection(State(shared), Path(id)).await
}
async fn restore_collection(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    let restored_ids = shared
        .bib
        .change_collection(&id, CollectionChange::Restore)
        .await
        .map_err(collection_error)?;
    Ok(Json(json!({"restored_ids":restored_ids})))
}

#[derive(Deserialize)]
struct CreateCollectionRequest {
    name: String,
    parent_id: Option<String>,
    description: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

async fn create_collection(
    State(shared): State<Arc<BibShared>>,
    Json(input): Json<CreateCollectionRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<ApiError>)> {
    if input.name.trim().is_empty() {
        return Err(error(StatusCode::BAD_REQUEST, "name is required"));
    }
    let id = format!("col-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let mut collection = Collection::new(&id, input.name.trim());
    collection.description = clean_optional(input.description);
    collection.tags = input.tags;
    collection.parent_id = input.parent_id;
    shared
        .bib
        .upsert_collection(&collection)
        .await
        .map_err(collection_error)?;
    let collection = shared
        .bib
        .get_collection(&id)
        .await
        .map_err(collection_error)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "collection not found"))?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "collection": collection })),
    ))
}

async fn get_collection(State(shared): State<Arc<BibShared>>, Path(id): Path<String>) -> ApiResult {
    let collection = shared
        .bib
        .get_collection(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("collection {id} not found")))?;
    Ok(Json(json!({ "collection": collection })))
}

async fn delete_collection(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let permanent = bool_param(&params, "permanent")?;
    if permanent {
        let col = shared
            .bib
            .list_collections_including_deleted(None, true)
            .await
            .map_err(collection_error)?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| error(StatusCode::NOT_FOUND, "collection not found"))?;
        if !col.deleted {
            return Err(error(
                StatusCode::CONFLICT,
                "move collection to trash before permanently erasing it",
            ));
        }
    }
    let ids = shared
        .bib
        .change_collection(
            &id,
            if permanent {
                CollectionChange::Erase
            } else {
                CollectionChange::Trash
            },
        )
        .await
        .map_err(collection_error)?;
    Ok(Json(
        json!({"deleted":true,"id":id,"deleted_ids":ids,"permanent":permanent}),
    ))
}

async fn list_collection_articles(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let limit = parse_limit(params.get("limit").map(String::as_str), 500)?;
    let offset = parse_offset(params.get("offset").map(String::as_str))?;
    let page = shared
        .bib
        .query_collection_articles(CollectionQuery {
            collection_id: Some(id),
            include_descendants: bool_param(&params, "include_descendants")?,
            unfiled: false,
            query: params.get("query").cloned().unwrap_or_default(),
            limit,
            offset,
        })
        .await
        .map_err(collection_error)?;
    let attachment_summaries = attachment_summary_map(&shared, &page.articles).await?;
    Ok(Json(
        json!({"total":page.total,"articles":page.articles,"associations":page.associations,"attachment_summaries":attachment_summaries}),
    ))
}

#[derive(Deserialize)]
struct AddToCollectionRequest {
    article_id: String,
    role: Option<String>,
    note: Option<String>,
}

async fn add_to_collection(
    State(shared): State<Arc<BibShared>>,
    Path(collection_id): Path<String>,
    Json(input): Json<AddToCollectionRequest>,
) -> ApiResult {
    shared
        .bib
        .get_collection(&collection_id)
        .await
        .map_err(collection_error)?
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                format!("collection {collection_id} not found"),
            )
        })?;
    shared
        .bib
        .get_article(&input.article_id)
        .await
        .map_err(collection_error)?
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                format!("article {} not found", input.article_id),
            )
        })?;

    let role = parse_article_role(input.role.as_deref());
    let outcome = if input.role.is_none() && input.note.is_none() {
        shared
            .bib
            .ensure_collection_membership(&collection_id, &input.article_id)
            .await
    } else {
        shared
            .bib
            .add_to_collection(
                &collection_id,
                &input.article_id,
                role,
                AddedBy::User,
                input.note.as_deref(),
            )
            .await
    }
    .map_err(collection_error)?;
    let inserted = outcome.was_inserted();
    Ok(Json(json!({
        "collection_id": collection_id,
        "article_id": input.article_id,
        "inserted": inserted,
    })))
}

fn parse_article_role(role: Option<&str>) -> ArticleRole {
    match role.map(str::to_ascii_lowercase).as_deref() {
        Some("requested") => ArticleRole::Requested,
        Some("cited") => ArticleRole::Cited,
        Some("background") => ArticleRole::Background,
        _ => ArticleRole::Referenced,
    }
}

async fn remove_from_collection(
    State(shared): State<Arc<BibShared>>,
    Path((collection_id, article_id)): Path<(String, String)>,
) -> ApiResult {
    shared
        .bib
        .remove_from_collection(&collection_id, &article_id)
        .await
        .map_err(collection_error)?;
    Ok(Json(json!({
        "removed": true,
        "collection_id": collection_id,
        "article_id": article_id,
    })))
}

#[derive(Deserialize)]
struct UpdateCollectionStatusRequest {
    status: String,
}

async fn update_collection_status(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<UpdateCollectionStatusRequest>,
) -> ApiResult {
    let status = match input.status.as_str() {
        "active" => CollectionStatus::Active,
        "completed" => CollectionStatus::Completed,
        "archived" => CollectionStatus::Archived,
        other => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                format!("unknown collection status '{other}'"),
            ));
        }
    };
    shared
        .bib
        .update_collection_status(&id, status)
        .await
        .map_err(collection_error)?;
    Ok(Json(json!({ "id": id, "status": input.status })))
}

async fn list_requests(
    State(shared): State<Arc<BibShared>>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let collection_id = params.get("collection_id").map(String::as_str);
    let requests = shared
        .bib
        .list_fulltext_requests(collection_id)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "requests": requests })))
}

async fn export(
    State(shared): State<Arc<BibShared>>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let format = match params
        .get("format")
        .map(String::as_str)
        .unwrap_or("bibtex")
        .to_ascii_lowercase()
        .as_str()
    {
        "ris" => ExportFormat::Ris,
        "markdown" | "md" => ExportFormat::Markdown,
        "csl_json" | "csljson" | "json" => ExportFormat::CslJson,
        _ => ExportFormat::Bibtex,
    };
    let limit = if params.contains_key("limit") {
        parse_limit(params.get("limit").map(String::as_str), 500)?
    } else {
        i64::MAX as usize
    };

    let articles = if let Some(collection_id) = params.get("collection_id") {
        shared
            .bib
            .query_collection_articles(CollectionQuery {
                collection_id: Some(collection_id.clone()),
                include_descendants: bool_param(&params, "include_descendants")?,
                unfiled: false,
                query: params.get("query").cloned().unwrap_or_default(),
                limit,
                offset: 0,
            })
            .await
            .map_err(collection_error)?
            .articles
    } else if let Some(ids) = params.get("ids") {
        // Export exactly the requested records, in the order given (the
        // selection order in the UI). Unknown ids are skipped; `limit` does
        // not apply because the caller chose the set explicitly.
        let mut articles = Vec::new();
        for id in ids.split(',').map(str::trim).filter(|id| !id.is_empty()) {
            if let Some(article) = shared.bib.get_article(id).await.map_err(internal)? {
                articles.push(article);
            }
        }
        articles
    } else {
        shared
            .bib
            .query_collection_articles(CollectionQuery {
                collection_id: None,
                include_descendants: false,
                unfiled: bool_param(&params, "unfiled")?,
                query: params.get("query").cloned().unwrap_or_default(),
                limit,
                offset: 0,
            })
            .await
            .map_err(collection_error)?
            .articles
    };

    let rendered = bib_base::render_all(&articles, format);
    Ok(Json(json!({
        "format": format.extension(),
        "count": articles.len(),
        "export": rendered,
    })))
}

async fn search_external(
    State(shared): State<Arc<BibShared>>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let query = params
        .get("query")
        .map(|query| query.trim())
        .filter(|query| !query.is_empty())
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "query is required"))?
        .to_owned();
    let limit = parse_limit(params.get("limit").map(String::as_str), 20)?;
    let sources = params.get("sources").map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|source| !source.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    });
    let search = StructuredSearch {
        keywords: Some(vec![query]),
        ..Default::default()
    };
    let batches = shared
        .gateway
        .search_named(sources.as_deref(), &search, limit)
        .await;
    Ok(Json(json!({ "batches": batches })))
}

fn parse_range_header(
    headers: &HeaderMap,
    size: u64,
) -> Result<Option<(u64, u64)>, (StatusCode, Json<ApiError>)> {
    let Some(range) = headers.get(header::RANGE) else {
        return Ok(None);
    };
    let range = range
        .to_str()
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid Range header"))?;
    if size == 0 {
        return Err(error(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "cannot read a byte range from an empty file",
        ));
    }
    let specification = range
        .strip_prefix("bytes=")
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Range header must use bytes units"))?;
    if specification.contains(',') {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "multi-part Range requests are not supported",
        ));
    }

    let (start, end) = if let Some(suffix) = specification.strip_prefix('-') {
        let suffix: u64 = suffix
            .parse()
            .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid Range suffix length"))?;
        if suffix == 0 {
            return Err(error(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "empty Range suffix",
            ));
        }
        let suffix = suffix.min(size);
        (size - suffix, size - 1)
    } else {
        let (start, end) = specification
            .split_once('-')
            .ok_or_else(|| error(StatusCode::BAD_REQUEST, "Range header must contain '-'"))?;
        let start: u64 = start
            .parse()
            .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid Range start"))?;
        if start >= size {
            return Err(error(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "Range start is past the end of the file",
            ));
        }
        let end = if end.is_empty() {
            size - 1
        } else {
            end.parse::<u64>()
                .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid Range end"))?
                .min(size - 1)
        };
        if end < start {
            return Err(error(StatusCode::BAD_REQUEST, "Range end precedes start"));
        }
        (start, end)
    };
    Ok(Some((start, end)))
}

fn file_storage(
    shared: &BibShared,
) -> Result<&Arc<vfs::OpendalFileStorage>, (StatusCode, Json<ApiError>)> {
    shared.file_storage.as_ref().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "bibliography VFS storage is not configured",
        )
    })
}

fn storage_error(failure: opendal::Error) -> (StatusCode, Json<ApiError>) {
    error(StatusCode::INTERNAL_SERVER_ERROR, failure.to_string())
}

async fn write_stored_file(
    shared: &BibShared,
    path: &str,
    content: Vec<u8>,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    let storage = file_storage(shared)?;
    storage
        .write_bytes(path, content)
        .await
        .map_err(storage_error)?;
    Ok(())
}

async fn delete_stored_file(
    shared: &BibShared,
    path: &str,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    let storage = file_storage(shared)?;
    storage.delete_object(path).await.map_err(storage_error)?;
    Ok(())
}

/// Best-effort removal of every markdown-derived artifact of an article:
/// the extracted-text object (`extracted/fulltext.{md|txt}`) and the
/// figure images its markdown references.
///
/// Called wherever the markdown that justifies those objects goes away:
/// upload-replace, delete, and re-extract (a fresh run rewrites the
/// figure set it needs; MinerU's content-derived names make that an
/// overwrite, so clearing first never loses anything a successful re-run
/// would reuse). Entirely advisory — a failed delete only leaves an
/// orphan behind.
async fn delete_article_figures(shared: &BibShared, fulltext: &FullText) {
    let Some(storage) = shared.file_storage.as_ref() else {
        return;
    };

    let object = bib_base::stored_files::structured_object_path(
        &fulltext.article_id,
        fulltext.attachment_key.as_deref(),
    );
    if let Some(path) = vfs_virtual_path(&object) {
        if let Err(failure) = storage.delete_object(&path).await {
            tracing::warn!(path = %path, error = %failure, "failed to remove Zotero structure");
        }
    }

    // The canonical extracted-text object goes away with its markdown,
    // whatever produced the row.
    if let Some(format) = fulltext.text_format {
        let extracted = extracted_object_path(&fulltext.article_id, format);
        if let Some(path) = vfs_virtual_path(&extracted) {
            if let Err(failure) = storage.delete_object(&path).await {
                tracing::warn!(path = %path, error = %failure, "failed to remove extracted text object");
            }
        }
    }

    if fulltext.text_format != Some(TextFormat::Markdown) {
        return;
    }
    // Scan the markdown for figure references: the canonical file is the
    // source of truth, with the row's inline copy as the fallback for
    // legacy rows the startup backfill has not materialized yet.
    let markdown = match vfs_virtual_path(&extracted_object_path(
        &fulltext.article_id,
        TextFormat::Markdown,
    )) {
        Some(path) => match read_full(storage, &path).await {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => fulltext.text_content.clone().unwrap_or_default(),
        },
        None => fulltext.text_content.clone().unwrap_or_default(),
    };
    if markdown.is_empty() {
        return;
    }
    for name in scan_image_refs(&markdown) {
        let Some(path) = vfs_virtual_path(&figure_object_path(&fulltext.article_id, &name)) else {
            continue;
        };
        if let Err(failure) = storage.delete_object(&path).await {
            tracing::warn!(path = %path, error = %failure, "failed to remove figure object");
        }
    }
}

/// Best-effort cleanup of one attachment's derived text and figures.
async fn delete_attachment_figures(
    shared: &BibShared,
    article_id: &str,
    attachment_key: &str,
    fulltext: &FullText,
) {
    let Some(storage) = shared.file_storage.as_ref() else {
        return;
    };
    let object = bib_base::stored_files::structured_object_path(article_id, Some(attachment_key));
    if let Some(path) = vfs_virtual_path(&object) {
        if let Err(failure) = storage.delete_object(&path).await {
            tracing::warn!(path = %path, error = %failure, "failed to remove attachment Zotero structure");
        }
    }
    let format = fulltext.text_format.unwrap_or_default();
    let extracted = extracted_attachment_object_path(article_id, attachment_key, format);
    if let Some(path) = vfs_virtual_path(&extracted) {
        if let Err(failure) = storage.delete_object(&path).await {
            tracing::warn!(path = %path, error = %failure, "failed to remove attachment extracted text");
        }
    }
    if fulltext.text_format != Some(TextFormat::Markdown) {
        return;
    }
    for name in scan_image_refs(fulltext.text_content.as_deref().unwrap_or_default()) {
        let object = attachment_figure_object_path(article_id, attachment_key, &name);
        let Some(path) = vfs_virtual_path(&object) else {
            continue;
        };
        if let Err(failure) = storage.delete_object(&path).await {
            tracing::warn!(path = %path, error = %failure, "failed to remove attachment figure");
        }
    }
}
