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
use bib_base::{
    BibShared, spawn_extraction, stored_fulltext, try_fetch_fulltext_with, vfs_virtual_path,
};
use bib_types::{
    AddedBy, AnnotationKind, Article, ArticleRole, ArticleSource, Author, Collection,
    CollectionStatus, ExportFormat, ExtractStatus, FileFormat, FullText, FullTextSource, IdKind,
    Identifier, StructuredSearch,
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
            "/articles/{id}/fulltext",
            get(get_fulltext)
                .post(upload_fulltext)
                .delete(delete_fulltext),
        )
        .route(
            "/articles/{id}/fulltext/raw",
            get(download_fulltext).head(download_fulltext),
        )
        .route(
            "/articles/{id}/fulltext/reextract",
            post(reextract_fulltext),
        )
        .route(
            "/articles/{id}/annotations",
            get(list_annotations).post(add_annotation),
        )
        .route("/annotations/{id}", delete(delete_annotation))
        .route(
            "/collections",
            get(list_collections).post(create_collection),
        )
        .route(
            "/collections/{id}",
            get(get_collection).delete(delete_collection),
        )
        .route(
            "/collections/{id}/articles",
            get(list_collection_articles).post(add_to_collection),
        )
        .route(
            "/collections/{id}/articles/{article_id}",
            delete(remove_from_collection),
        )
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

async fn list_articles(
    State(shared): State<Arc<BibShared>>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let query = params.get("query").map(String::as_str).unwrap_or("");
    let limit = parse_limit(params.get("limit").map(String::as_str), 100)?;
    let hits = shared
        .bib
        .search_articles(query, limit)
        .await
        .map_err(internal)?;
    let total = shared.bib.article_count().await.map_err(internal)?;
    let mut articles = Vec::with_capacity(hits.len());
    for hit in &hits {
        if let Some(article) = shared
            .bib
            .get_article(&hit.article_id)
            .await
            .map_err(internal)?
        {
            articles.push(article);
        }
    }
    Ok(Json(
        json!({ "total": total, "hits": hits, "articles": articles }),
    ))
}

async fn list_unfiled_articles(
    State(shared): State<Arc<BibShared>>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let limit = parse_limit(params.get("limit").map(String::as_str), 500)?;
    let collections = shared.bib.list_collections(None).await.map_err(internal)?;
    let filed_ids = collections
        .iter()
        .flat_map(|collection| collection.article_ids.iter())
        .collect::<std::collections::HashSet<_>>();
    let articles = shared
        .bib
        .list_all_articles()
        .await
        .map_err(internal)?
        .into_iter()
        .filter(|article| !filed_ids.contains(&article.id))
        .take(limit)
        .collect::<Vec<_>>();
    let total = articles.len();
    Ok(Json(json!({ "total": total, "articles": articles })))
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
    shared
        .bib
        .upsert_article(&article)
        .await
        .map_err(internal)?;

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
    id_type: String,
    id: String,
    source: Option<String>,
    fetch_fulltext: Option<bool>,
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
    let annotations = shared.bib.list_annotations(&id).await.map_err(internal)?;
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
        }
    }
    Ok(Json(
        json!({ "deleted": removed > 0, "id": id, "rows_removed": removed }),
    ))
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
    }
    // Content-addressed paths guarantee the queued task reads the object
    // just written, even though this handler returns immediately.
    spawn_extraction(shared.as_ref().clone(), article.id.clone());
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
    if !shared.bib.restart_extraction(&id).await.map_err(internal)? {
        return Err(error(
            StatusCode::NOT_FOUND,
            format!("full text for {id} not found"),
        ));
    }
    spawn_extraction(shared.as_ref().clone(), id.clone());
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
    }
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn list_annotations(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    let annotations = shared.bib.list_annotations(&id).await.map_err(internal)?;
    Ok(Json(json!({ "annotations": annotations })))
}

#[derive(Deserialize)]
struct AddAnnotationRequest {
    content: String,
    kind: Option<String>,
    page: Option<u32>,
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
        .add_annotation(&id, kind, input.content.trim(), input.page)
        .await
        .map_err(internal)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "annotation": annotation })),
    ))
}

async fn delete_annotation(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    shared.bib.delete_annotation(&id).await.map_err(internal)?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn list_collections(State(shared): State<Arc<BibShared>>) -> ApiResult {
    let collections = shared.bib.list_collections(None).await.map_err(internal)?;
    Ok(Json(json!({ "collections": collections })))
}

#[derive(Deserialize)]
struct CreateCollectionRequest {
    name: String,
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
    shared
        .bib
        .upsert_collection(&collection)
        .await
        .map_err(internal)?;
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
) -> ApiResult {
    shared.bib.delete_collection(&id).await.map_err(internal)?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn list_collection_articles(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    shared
        .bib
        .get_collection(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("collection {id} not found")))?;

    let associations = shared
        .bib
        .list_collection_articles(&id, None, None)
        .await
        .map_err(internal)?;
    let mut articles = Vec::with_capacity(associations.len());
    for association in &associations {
        articles.push(
            shared
                .bib
                .get_article(&association.article_id)
                .await
                .map_err(internal)?,
        );
    }
    Ok(Json(json!({
        "associations": associations,
        "articles": articles,
    })))
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
        .map_err(internal)?
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
        .map_err(internal)?
        .ok_or_else(|| {
            error(
                StatusCode::NOT_FOUND,
                format!("article {} not found", input.article_id),
            )
        })?;

    let role = parse_article_role(input.role.as_deref());
    let inserted = shared
        .bib
        .add_to_collection(
            &collection_id,
            &input.article_id,
            role,
            AddedBy::User,
            input.note.as_deref(),
        )
        .await
        .map_err(internal)?
        .was_inserted();
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
        .map_err(internal)?;
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
        .map_err(internal)?;
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
    let limit = parse_limit(params.get("limit").map(String::as_str), 500)?;

    let articles = if let Some(collection_id) = params.get("collection_id") {
        let associations = shared
            .bib
            .list_collection_articles(collection_id, None, None)
            .await
            .map_err(internal)?;
        let mut articles = Vec::new();
        for association in associations.into_iter().take(limit) {
            if let Some(article) = shared
                .bib
                .get_article(&association.article_id)
                .await
                .map_err(internal)?
            {
                articles.push(article);
            }
        }
        articles
    } else {
        shared
            .bib
            .list_all_articles()
            .await
            .map_err(internal)?
            .into_iter()
            .take(limit)
            .collect()
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
