use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::header,
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response, sse::Sse},
    routing::{delete, get, post, put},
};
use bib_base::{
    BibShared, ListParams, OcrFallbackExtractor, ParseEvent, SimpleExtractor, SortField, SortOrder,
    TextExtractor,
    import::{ImportFormat, parse_import},
    stored_fulltext, try_fetch_fulltext_with, vfs_virtual_path,
};
use bib_types::{
    AddedBy, AnnotationKind, Article, ArticleRole, ArticleSource, Author, Collection,
    CollectionStatus, ExportFormat, FileFormat, FullText, FullTextSource, IdKind, Identifier,
    SearchHit, StructuredSearch,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;
const DEFAULT_FULLTEXT_CHARS: usize = 100_000;
const MAX_FULLTEXT_CHARS: usize = 500_000;
/// Largest page the article listing will serve.
///
/// The library grid asks for a whole collection (or the whole library) in one
/// request, which is a different access pattern from the search endpoints
/// that stay on [`parse_limit`]'s 500-row ceiling.
const MAX_LIST_ARTICLES: usize = 2000;
/// Prefix under which every web-client setting is persisted in `bib_meta`.
const SETTINGS_PREFIX: &str = "web:";
/// Prefix under which chat transcripts are persisted in `bib_meta`.
const CHAT_PREFIX: &str = "web:chat:";
/// The slice of the `web:` namespace reserved for chat transcripts, relative
/// to [`SETTINGS_PREFIX`].
const CHAT_NAMESPACE: &str = "chat:";
/// Longest accepted chat scope. Scopes are opaque client-chosen labels, so
/// the only thing worth limiting is how much of the key space one can name.
const MAX_CHAT_SCOPE_BYTES: usize = 512;
/// Largest chat transcript accepted by `POST /chat`.
///
/// The chat client resends the whole conversation on every turn, so the
/// ceiling must be generous — but each transcript lands in a single
/// `bib_meta` row that is read back in full by the settings scan, so it
/// cannot be unbounded.
const MAX_CHAT_PAYLOAD_BYTES: usize = 5 * 1024 * 1024;
/// Deepest `parent_id` chain the cycle check will follow.
///
/// Collections written before `parent_id` existed cannot contain a cycle, but
/// a hand-edited database could; the cap turns that into a 400 instead of a
/// request that never returns. Sixty-four levels is already far deeper than
/// any usable taxonomy.
const MAX_COLLECTION_DEPTH: usize = 64;

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

pub(crate) fn router(shared: BibShared) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/articles", get(list_articles).post(create_article))
        .route("/articles/unfiled", get(list_unfiled_articles))
        .route("/articles/upload", post(upload_article))
        .route("/articles/import", post(import_article))
        .route("/articles/import/batch", post(import_batch))
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
            "/articles/{id}/annotations",
            get(list_annotations).post(add_annotation),
        )
        .route("/articles/{id}/csl-json", get(get_article_csl_json))
        .route(
            "/articles/{id}/fetch-metrics",
            post(fetch_article_metrics),
        )
        .route("/articles/{id}/reparse", post(reparse_paper))
        .route("/articles/{id}/parse/stream", get(parse_stream))
        .route("/fulltext-statuses", get(get_fulltext_statuses))
        .route(
            "/annotations/{id}",
            put(update_annotation).delete(delete_annotation),
        )
        .route(
            "/collections",
            get(list_collections).post(create_collection),
        )
        .route(
            "/collections/{id}",
            get(get_collection)
                .put(update_collection)
                .delete(delete_collection),
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
        .route("/journals/metrics", get(list_journal_metrics))
        .route("/journals/validate-easyscholar", get(validate_easyscholar))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/chat", get(get_chat).post(post_chat))
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
    parse_limit_max(raw, default, 500)
}

/// Like [`parse_limit`] but with a caller-chosen ceiling.
///
/// The old body hard-coded `clamp(1, 500)`; the library listing needed a
/// higher ceiling without changing the behaviour of every other endpoint
/// that shares this helper.
fn parse_limit_max(
    raw: Option<&str>,
    default: usize,
    max: usize,
) -> Result<usize, (StatusCode, Json<ApiError>)> {
    let value = raw
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default);
    if value == 0 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "limit must be greater than 0",
        ));
    }
    Ok(value.clamp(1, max))
}

fn parse_offset(raw: Option<&str>) -> Result<usize, (StatusCode, Json<ApiError>)> {
    match raw {
        None => Ok(0),
        // No upper bound: an offset past the end is a legitimate empty page,
        // and clamping it would make two different requests indistinguishable.
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|_| error(StatusCode::BAD_REQUEST, "offset must be a whole number")),
    }
}

/// Deserialize `Option<Option<T>>` so an absent JSON field means "leave
/// unchanged" and an explicit `null` means "clear the value".
///
/// Serde maps both to `None` by default, which would make it impossible to
/// unset `parent_id` or `page` through the partial-update endpoints. This is
/// the well-known `serde_with::rust::double_option` behaviour, inlined to
/// avoid a new dependency for eight lines.
fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

/// One row of `GET /articles`: the stored [`Article`] flattened as-is, plus
/// a list-only `has_fulltext` flag. The web client gates its per-row
/// "attach file" context-menu action on it, while the Article model stays
/// free of any full-text fields.
#[derive(Serialize)]
struct ArticleListEntry {
    #[serde(flatten)]
    article: Article,
    has_fulltext: bool,
}

async fn list_articles(
    State(shared): State<Arc<BibShared>>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let query = params.get("query").cloned().unwrap_or_default();
    let limit = parse_limit_max(
        params.get("limit").map(String::as_str),
        100,
        MAX_LIST_ARTICLES,
    )?;
    let offset = parse_offset(params.get("offset").map(String::as_str))?;
    let sort = match params.get("sort").map(String::as_str) {
        None => SortField::CreatedAt,
        Some(raw) => SortField::from_wire(raw).ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "sort must be one of created_at, updated_at, title, year",
            )
        })?,
    };
    let order = match params.get("order").map(String::as_str) {
        None => SortOrder::Descending,
        Some(raw) => SortOrder::from_wire(raw)
            .ok_or_else(|| error(StatusCode::BAD_REQUEST, "order must be asc or desc"))?,
    };
    let collection_id = params
        .get("collection_id")
        .filter(|id| !id.is_empty())
        .cloned();
    if let Some(id) = &collection_id {
        // 404 rather than a silent empty page: a stale collection ID in a
        // saved view should look like a mistake, not like "nothing filed yet".
        shared
            .bib
            .get_collection(id)
            .await
            .map_err(internal)?
            .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("collection {id} not found")))?;
    }
    let unfiled = match params.get("unfiled").map(String::as_str) {
        None | Some("") | Some("false") => false,
        Some("true") => true,
        Some(other) => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                format!("unfiled must be true or false, got '{other}'"),
            ));
        }
    };

    let (articles, total) = shared
        .bib
        .list_articles_paged(&ListParams {
            query,
            offset,
            limit,
            sort,
            order,
            collection_id: collection_id.clone(),
            unfiled,
        })
        .await
        .map_err(internal)?;

    // `hits` is kept for clients written against the pre-pagination shape.
    // The page is now ordered by the caller's sort key, so the search
    // relevance score no longer applies and the snippet degrades to the
    // title (what an empty query already showed).
    let hits = articles
        .iter()
        .map(|article| SearchHit {
            article_id: article.id.clone(),
            title: article.title.clone(),
            score: 0.0,
            snippet: article.title.clone(),
        })
        .collect::<Vec<_>>();

    let fulltext_ids = shared
        .bib
        .list_fulltext_article_ids()
        .await
        .map_err(internal)?;
    let articles = articles
        .into_iter()
        .map(|article| ArticleListEntry {
            has_fulltext: fulltext_ids.contains(&article.id),
            article,
        })
        .collect::<Vec<_>>();

    Ok(Json(json!({
        "total": total,
        "hits": hits,
        "articles": articles,
        "offset": offset,
        "limit": limit,
    })))
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

    // Journal-metrics enrichment is fire-and-forget — see
    // [`spawn_journal_metrics_enrichment`].
    if let Some(journal) = article_journal_name(&article) {
        spawn_journal_metrics_enrichment(&shared, vec![journal]);
    }

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

/// How much of the extracted text is scanned for a DOI / arXiv id. Covers a
/// paper's first page, where both are printed.
const IDENTIFIER_SCAN_CHARS: usize = 4000;

/// Upload a document file and create the article for it in one step.
///
/// The extracted text is scanned for a DOI or arXiv id: a hit fetches real
/// metadata from the gateway (falling back to an identifier-keyed stub when
/// no source answers), a miss creates a manual `local:{uuid}` record named
/// after the file. Re-uploading a paper that is already in the library
/// attaches the file to the existing article instead of duplicating it.
async fn upload_article(
    State(shared): State<Arc<BibShared>>,
    mut multipart: Multipart,
) -> ApiResult {
    let mut filename = None;
    let mut content = None;
    let mut category_id = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|multipart_error| error(StatusCode::BAD_REQUEST, multipart_error.to_string()))?
    {
        match field.name() {
            Some("file") => {
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
            }
            Some("category_id") => {
                category_id = Some(field.text().await.map_err(|multipart_error| {
                    error(StatusCode::BAD_REQUEST, multipart_error.to_string())
                })?);
            }
            _ => {}
        }
    }

    let Some(content) = content else {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "multipart field 'file' is required",
        ));
    };
    let filename = filename.unwrap_or_else(|| "upload.txt".to_owned());

    // Validate the category before any extraction work so a typo fails fast.
    if let Some(category) = category_id.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        shared
            .bib
            .get_collection(category)
            .await
            .map_err(internal)?
            .ok_or_else(|| {
                error(
                    StatusCode::NOT_FOUND,
                    format!("collection {category} not found"),
                )
            })?;
    }

    let extension = filename
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .unwrap_or("txt");
    let format = FileFormat::from_extension(extension);
    let is_pdf = format == FileFormat::Pdf;

    // PDFs parse asynchronously through MinerU: this request only needs a
    // quick local extraction to look for a DOI / arXiv id (no OCR — a
    // scanned first page simply falls back to a manual record), while the
    // markdown itself arrives later over the parse stream. Without a token
    // there is nothing to parse with, so fail up front rather than storing
    // an unparseable PDF. Html/Txt stay on the synchronous local path.
    let extracted_text = if is_pdf {
        ensure_mineru_token(&shared)?;
        match SimpleExtractor::new().extract(&content, format).await {
            Ok(text) => Some(text.text),
            Err(_) => None, // identifier matching is best-effort
        }
    } else {
        match OcrFallbackExtractor::new().extract(&content, format).await {
            Ok(text) => Some(text.text),
            Err(extract_error) => {
                tracing::warn!(
                    filename = %filename,
                    error = %extract_error,
                    "failed to extract upload content; storing the original bytes only"
                );
                None
            }
        }
    };

    let matched = extracted_text
        .as_deref()
        .and_then(find_identifier_in_text);

    let (article, created) = match &matched {
        Some(identifier) => {
            if let Some(existing) = shared
                .bib
                .find_by_identifier(identifier.kind, &identifier.value)
                .await
                .map_err(internal)?
            {
                (existing, false)
            } else if let Some((_source, mut fetched)) = shared.gateway.fetch(identifier).await {
                fetched.updated_at = Some(Utc::now());
                (fetched, true)
            } else {
                // No source answered (offline, or unknown id): key the stub by
                // the identifier so a later metadata refresh can upsert it.
                (stub_from_identifier(identifier, &filename), true)
            }
        }
        None => {
            let mut article =
                Article::new(format!("local:{}", Uuid::new_v4()), filename_stem(&filename));
            article.source = ArticleSource::Manual;
            (article, true)
        }
    };

    if created {
        shared
            .bib
            .upsert_article(&article)
            .await
            .map_err(internal)?;
    }
    let fulltext = store_fulltext_file(
        &shared,
        &article.id,
        &filename,
        &content,
        format,
        if is_pdf {
            None
        } else {
            extracted_text.clone()
        },
        if is_pdf { "pending" } else { "done" },
        if is_pdf { None } else { Some("builtin") },
    )
    .await?;
    if is_pdf {
        shared.parse_hub.spawn((*shared).clone(), article.id.clone());
    }

    if let Some(category) = category_id.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        shared
            .bib
            .add_to_collection(
                category,
                &article.id,
                ArticleRole::Referenced,
                AddedBy::User,
                None,
            )
            .await
            .map_err(internal)?;
    }

    Ok(Json(json!({
        "created": created,
        "article": article,
        "fulltext": fulltext,
        "identifier": matched.map(|identifier| json!({
            "kind": identifier.kind.as_str(),
            "value": identifier.value,
        })),
        "text_chars": if is_pdf {
            0
        } else {
            extracted_text.map(|text| text.chars().count()).unwrap_or(0)
        },
    })))
}

/// Minimal article for an upload whose identifier no gateway source could
/// resolve. The title is just the filename until metadata arrives.
fn stub_from_identifier(identifier: &Identifier, filename: &str) -> Article {
    let mut article = Article::new(
        format!("{}:{}", identifier.kind.as_str(), identifier.value),
        filename_stem(filename),
    );
    article.identifiers.push(identifier.clone());
    article.source = ArticleSource::Manual;
    article
}

/// Human-ish title for an uploaded file: the name without its extension.
fn filename_stem(filename: &str) -> String {
    let stem = filename
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(filename)
        .trim();
    if stem.is_empty() {
        "Untitled upload".to_owned()
    } else {
        stem.to_owned()
    }
}

/// Find a DOI (`10.NNNN/…`) or arXiv id in the head of the extracted text.
fn find_identifier_in_text(text: &str) -> Option<Identifier> {
    let head: String = text.chars().take(IDENTIFIER_SCAN_CHARS).collect();
    find_doi(&head)
        .map(Identifier::doi)
        .or_else(|| find_arxiv(&head).map(|id| Identifier::new(IdKind::Arxiv, id)))
}

/// Scan for `10.\d{4,9}/suffix`, hand-rolled to stay dependency-free.
fn find_doi(head: &str) -> Option<String> {
    let bytes = head.as_bytes();
    let mut cursor = 0;
    while let Some(offset) = head[cursor..].find("10.") {
        let start = cursor + offset;
        // `210.12345` or `A10.1` are not DOIs: require a word boundary.
        let boundary_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let mut rest = start + 3;
        let mut digits = 0;
        while rest < bytes.len() && bytes[rest].is_ascii_digit() {
            digits += 1;
            rest += 1;
        }
        if boundary_ok && (4..=9).contains(&digits) && rest < bytes.len() && bytes[rest] == b'/' {
            let suffix_start = rest + 1;
            let mut end = suffix_start;
            // The suffix is printable ASCII without whitespace.
            while end < bytes.len() && (0x21..=0x7e).contains(&bytes[end]) {
                end += 1;
            }
            // Sentence punctuation after a DOI is prose, not part of it.
            while suffix_start < end
                && matches!(
                    bytes[end - 1],
                    b'.' | b',' | b';' | b':' | b')' | b']' | b'}' | b'"' | b'\''
                )
            {
                end -= 1;
            }
            if end > suffix_start {
                // The whole DOI includes the `10.NNNN/` prefix.
                return Some(head[start..end].to_owned());
            }
        }
        cursor = start + 3;
    }
    None
}

/// Scan for an arXiv reference: `arXiv:2401.12345`, `arXiv/…`,
/// `arxiv.org/abs/2401.12345v2`.
fn find_arxiv(head: &str) -> Option<String> {
    let lowered = head.to_lowercase();
    let mut cursor = 0;
    while let Some(offset) = lowered[cursor..].find("arxiv") {
        let after = cursor + offset + "arxiv".len();
        // The id sits within the next few dozen characters of the mention.
        let window: String = lowered[after..].chars().take(40).collect();
        if let Some(id_start) = window.find(|c: char| c.is_ascii_digit()) {
            if let Some(id) = take_arxiv_id(&window[id_start..]) {
                return Some(id);
            }
        }
        cursor = after;
    }
    None
}

/// `dddd.ddddd` (4–5 digit sequence), with an optional `vN` suffix.
fn take_arxiv_id(rest: &str) -> Option<String> {
    let bytes = rest.as_bytes();
    let mut index = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index != 4 || index >= bytes.len() || bytes[index] != b'.' {
        return None;
    }
    index += 1;
    let sequence_start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if !(4..=5).contains(&(index - sequence_start)) {
        return None;
    }
    let mut end = index;
    if end + 1 < bytes.len() && bytes[end] == b'v' && bytes[end + 1].is_ascii_digit() {
        end += 2;
    }
    Some(rest[..end].to_owned())
}

#[derive(Deserialize)]
struct ImportBatchRequest {
    /// `bibtex` | `ris` | `csl_json` | `auto`.
    format: String,
    content: String,
    category_id: Option<String>,
}

/// Import a bibliography file (BibTeX / RIS / CSL-JSON) in one request.
///
/// Entries whose identifiers are already stored — or appear twice in the
/// batch — are reported as duplicates rather than re-imported; entries that
/// cannot yield an article (no title) come back in `failed` with a reason.
async fn import_batch(
    State(shared): State<Arc<BibShared>>,
    Json(input): Json<ImportBatchRequest>,
) -> ApiResult {
    let format = match input.format.as_str() {
        "auto" => ImportFormat::sniff(&input.content).ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "could not detect the import format; pass format explicitly",
            )
        })?,
        name => ImportFormat::from_name(name).ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                "format must be bibtex, ris, csl_json, or auto",
            )
        })?,
    };

    if let Some(category) = input.category_id.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        shared
            .bib
            .get_collection(category)
            .await
            .map_err(internal)?
            .ok_or_else(|| {
                error(StatusCode::NOT_FOUND, format!("collection {category} not found"))
            })?;
    }

    let (candidates, failures) = parse_import(format, &input.content);

    // Dedup against stored identifiers and within the batch itself. The
    // batch map keys identifiers of freshly accepted entries so a later
    // duplicate points at the id it was merged into.
    let mut fresh: Vec<Article> = Vec::new();
    let mut duplicates: Vec<Value> = Vec::new();
    let mut batch_identifiers: HashMap<(String, String), String> = HashMap::new();
    for article in candidates {
        let mut duplicate_of: Option<String> = None;
        for identifier in &article.identifiers {
            if let Some(existing) = shared
                .bib
                .find_by_identifier(identifier.kind, &identifier.value)
                .await
                .map_err(internal)?
            {
                duplicate_of = Some(existing.id);
                break;
            }
        }
        if duplicate_of.is_none() {
            for identifier in &article.identifiers {
                let key = (
                    identifier.kind.as_str().to_owned(),
                    identifier.value.clone(),
                );
                if let Some(earlier) = batch_identifiers.get(&key) {
                    duplicate_of = Some(earlier.clone());
                    break;
                }
            }
        }

        match duplicate_of {
            Some(existing_id) => duplicates.push(json!({
                "title": article.title,
                "identifiers": article.identifiers,
                "existing_id": existing_id,
            })),
            None => {
                for identifier in &article.identifiers {
                    batch_identifiers.insert(
                        (identifier.kind.as_str().to_owned(), identifier.value.clone()),
                        article.id.clone(),
                    );
                }
                fresh.push(article);
            }
        }
    }

    if !fresh.is_empty() {
        shared
            .bib
            .upsert_articles(&fresh)
            .await
            .map_err(internal)?;
        // Batch imports are the main source of new journals — enrich them
        // all in one background pass (per-journal dedup happens inside).
        let journals = fresh
            .iter()
            .filter_map(article_journal_name)
            .collect::<Vec<_>>();
        spawn_journal_metrics_enrichment(&shared, journals);
    }
    if let Some(category) = input.category_id.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        for article in &fresh {
            shared
                .bib
                .add_to_collection(
                    category,
                    &article.id,
                    ArticleRole::Referenced,
                    AddedBy::User,
                    None,
                )
                .await
                .map_err(internal)?;
        }
    }

    Ok(Json(json!({
        "imported": fresh.len(),
        "duplicates": duplicates.len(),
        "failed": failures
            .iter()
            .map(|failure| json!({ "key": failure.key, "reason": failure.reason }))
            .collect::<Vec<_>>(),
        "duplicate_details": duplicates,
        "articles": fresh,
    })))
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
    // A metadata edit may have introduced or corrected the journal name —
    // enrich in the background if that journal is not cached yet.
    if let Some(journal) = article_journal_name(&article) {
        spawn_journal_metrics_enrichment(&shared, vec![journal]);
    }
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
    let is_pdf = format == FileFormat::Pdf;

    // Same split as `POST /articles/upload`: PDFs parse async via MinerU,
    // everything else extracts synchronously and is readable immediately.
    if is_pdf {
        ensure_mineru_token(&shared)?;
    }
    let extracted_text = if is_pdf {
        None
    } else {
        match OcrFallbackExtractor::new().extract(&content, format).await {
            Ok(text) => Some(text.text),
            Err(extract_error) => {
                tracing::warn!(
                    article_id = %article.id,
                    filename = %filename,
                    error = %extract_error,
                    "failed to extract full-text content; storing original bytes only"
                );
                None
            }
        }
    };

    let fulltext = store_fulltext_file(
        &shared,
        &article.id,
        &filename,
        &content,
        format,
        extracted_text,
        if is_pdf { "pending" } else { "done" },
        if is_pdf { None } else { Some("builtin") },
    )
    .await?;
    if is_pdf {
        shared.parse_hub.spawn((*shared).clone(), article.id.clone());
    }
    Ok(Json(json!({ "fulltext": fulltext })))
}

/// Reject a PDF upload before anything is stored when no MinerU token is
/// configured. The async pipeline has no local fallback, so a stored-but-
/// unparseable PDF would just sit at `pending` forever.
fn ensure_mineru_token(
    shared: &Arc<BibShared>,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    if shared.parse_hub.mineru.has_key() {
        return Ok(());
    }
    Err(error(
        StatusCode::BAD_REQUEST,
        "PDF parsing requires a MinerU API token — set mineru_key in settings \
         (PUT /settings) or the MINERU_API_TOKEN environment variable",
    ))
}

/// Write uploaded bytes into the VFS and record the full-text row.
///
/// Split out of the article-scoped upload endpoint so `POST /articles/upload`
/// (which creates the article itself) shares the same storage path. On a
/// database rejection the freshly written object is removed again — unless it
/// occupies the path the previous row still references, in which case the old
/// object must survive. A replaced row's old object is cleaned up on success.
///
/// `parse_status` / `parse_engine` carry the async-pipeline state: `pending`
/// for PDFs awaiting MinerU, `done` + `builtin` for synchronously extracted
/// formats.
async fn store_fulltext_file(
    shared: &Arc<BibShared>,
    article_id: &str,
    filename: &str,
    content: &Bytes,
    format: FileFormat,
    extracted_text: Option<String>,
    parse_status: &str,
    parse_engine: Option<&str>,
) -> Result<FullText, (StatusCode, Json<ApiError>)> {
    let previous = shared
        .bib
        .get_fulltext(article_id)
        .await
        .map_err(internal)?;
    let stored = stored_fulltext(article_id, filename, content);
    let path = vfs_virtual_path(&stored.path).expect("stored fulltext uses a VFS path");
    write_stored_file(shared, &path, content.to_vec()).await?;
    let fulltext = FullText {
        article_id: article_id.to_owned(),
        file_path: stored.path,
        file_format: format,
        text_content: extracted_text,
        source: FullTextSource::UserUpload,
        file_hash: Some(stored.file_hash),
        file_size: Some(content.len() as i64),
        uploaded_at: Some(Utc::now()),
        parse_status: parse_status.to_owned(),
        parse_engine: parse_engine.map(str::to_owned),
        parse_error: None,
    };
    if let Err(db_error) = shared.bib.upsert_fulltext(&fulltext).await {
        if previous.as_ref().map(|old| old.file_path.as_str()) != Some(fulltext.file_path.as_str())
        {
            if let Err(cleanup_error) = delete_stored_file(shared, &path).await {
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
                if let Err(cleanup_error) = delete_stored_file(shared, &path).await {
                    tracing::warn!(
                        path = %path,
                        error = %cleanup_error.1.error,
                        "old full-text object remains after successful database update"
                    );
                }
            }
        }
    }
    Ok(fulltext)
}

/// Re-run the MinerU parse for a stored full text (the list view's
/// "re-parse" menu entry). The row flips to `pending` and a background task
/// takes over; progress arrives over `GET /articles/{id}/parse/stream`.
async fn reparse_paper(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    shared
        .bib
        .get_article(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("article {id} not found")))?;
    let fulltext = shared
        .bib
        .get_fulltext(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("no full text for {id}")))?;
    if vfs_virtual_path(&fulltext.file_path).is_none() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "original file is not stored in the VFS; nothing to re-parse",
        ));
    }
    ensure_mineru_token(&shared)?;
    if shared.parse_hub.is_running(&id) {
        return Err(error(
            StatusCode::CONFLICT,
            "a parse is already running for this article",
        ));
    }
    shared
        .bib
        .set_parse_status(&id, "pending", None)
        .await
        .map_err(internal)?;
    shared.parse_hub.spawn((*shared).clone(), id.clone());
    Ok(Json(json!({ "id": id, "message": "re-parse started" })))
}

/// SSE stream of one article's parse pipeline: `progress` / `done` / `error`
/// events with jayread's payload shapes (no paper id inside the data — the
/// URL already names the article), plus 10 s pings to keep proxies open.
///
/// Opening the stream also lazily resumes a parse the DB claims is in flight
/// but that lost its task to a restart.
async fn parse_stream(State(shared): State<Arc<BibShared>>, Path(id): Path<String>) -> Response {
    let receiver = shared.parse_hub.subscribe(&id);
    shared.parse_hub.ensure_running(&shared, &id).await;

    // Race cover: the parse may have finished between `subscribe` pruning
    // the channel and this read. Synthesize the terminal event from the
    // stored row so a late subscriber still learns the outcome.
    let snapshot = shared.bib.get_fulltext(&id).await.ok().flatten();
    let terminal = if shared.parse_hub.is_running(&id) {
        None
    } else {
        snapshot.as_ref().and_then(|ft| match ft.parse_status.as_str() {
            "done" => Some(ParseEvent::Done {
                parse_engine: ft
                    .parse_engine
                    .clone()
                    .unwrap_or_else(|| "builtin".to_owned()),
                markdown_length: ft
                    .text_content
                    .as_deref()
                    .map(|text| text.chars().count())
                    .unwrap_or(0),
            }),
            "failed" => Some(ParseEvent::Error {
                message: ft
                    .parse_error
                    .clone()
                    .unwrap_or_else(|| "parse failed".to_owned()),
            }),
            _ => None,
        })
    };

    let mut ping = tokio::time::interval(std::time::Duration::from_secs(10));
    // `interval` fires its first tick immediately; consume it so the stream
    // opens with real content (or silence), not a synthetic ping.
    ping.tick().await;

    let stream = futures::stream::unfold(
        ParseStreamState {
            receiver,
            ping,
            terminal,
            finished: false,
        },
        |mut state| async move {
            if state.finished {
                return None;
            }
            if let Some(event) = state.terminal.take() {
                state.finished = true;
                return Some((Ok::<_, std::convert::Infallible>(parse_sse(&event)), state));
            }
            loop {
                let event = tokio::select! {
                    maybe = state.receiver.recv() => match maybe {
                        Ok(event) => event,
                        // Missed intermediate progress under a slow consumer;
                        // the terminal event is what matters, keep listening.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        // Every sender is gone (the hub pruned the channel):
                        // end cleanly without inventing an outcome. (No
                        // `finished` flip needed — returning None already
                        // terminates the stream and drops the state.)
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return None;
                        }
                    },
                    _ = state.ping.tick() => {
                        return Some((
                            Ok::<_, std::convert::Infallible>(crate::agent::sse("ping", json!({}))),
                            state,
                        ));
                    }
                };
                if matches!(event, ParseEvent::Done { .. } | ParseEvent::Error { .. }) {
                    state.finished = true;
                }
                return Some((Ok::<_, std::convert::Infallible>(parse_sse(&event)), state));
            }
        },
    );

    let mut response = Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response();
    // Belt-and-braces alongside KeepAlive: an explicit no-cache so dev
    // proxies do not buffer the stream.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-cache"),
    );
    response
}

/// unfold state for [`parse_stream`].
struct ParseStreamState {
    receiver: tokio::sync::broadcast::Receiver<ParseEvent>,
    ping: tokio::time::Interval,
    /// Terminal event synthesized from the DB when the stream opens after a
    /// parse already settled.
    terminal: Option<ParseEvent>,
    finished: bool,
}

/// Render a [`ParseEvent`] as an SSE frame (reuses the agent endpoint's
/// JSON-payload helper).
fn parse_sse(event: &ParseEvent) -> axum::response::sse::Event {
    crate::agent::sse(event.event_name(), event)
}

/// Parse status of every stored full text. The web list view joins this
/// client-side over `/articles`, mirroring `/collections` and
/// `/journals/metrics`.
async fn get_fulltext_statuses(State(shared): State<Arc<BibShared>>) -> ApiResult {
    let statuses = shared
        .bib
        .list_parse_statuses()
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "statuses": statuses })))
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
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let mut annotations = shared.bib.list_annotations(&id).await.map_err(internal)?;
    // Filtered here rather than in SQL: an article has at most a few hundred
    // annotations, and keeping `list_annotations` parameter-free keeps it
    // usable from every other caller.
    if let Some(raw) = params.get("page").map(String::as_str) {
        let page: u32 = raw
            .parse()
            .map_err(|_| error(StatusCode::BAD_REQUEST, "page must be a whole number"))?;
        // `Annotation.page` is 1-based, so the query parameter matches it.
        annotations.retain(|annotation| annotation.page == Some(page));
    }
    Ok(Json(json!({ "annotations": annotations })))
}

#[derive(Deserialize)]
struct AddAnnotationRequest {
    content: String,
    kind: Option<String>,
    page: Option<u32>,
    /// Kind-specific payload, e.g. highlight rectangles and colour.
    #[serde(default)]
    data: Option<Value>,
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
        .add_annotation(&id, kind, input.content.trim(), input.page, input.data)
        .await
        .map_err(internal)?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "annotation": annotation })),
    ))
}

#[derive(Deserialize)]
struct UpdateAnnotationRequest {
    content: Option<String>,
    /// `null` clears the page anchor; omitting the field leaves it alone.
    #[serde(default, deserialize_with = "double_option")]
    page: Option<Option<u32>>,
    /// `null` clears the payload; omitting the field leaves it alone.
    #[serde(default, deserialize_with = "double_option")]
    data: Option<Option<Value>>,
}

async fn update_annotation(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<UpdateAnnotationRequest>,
) -> ApiResult {
    if let Some(content) = &input.content {
        if content.trim().is_empty() {
            return Err(error(StatusCode::BAD_REQUEST, "content cannot be empty"));
        }
    }
    let annotation = shared
        .bib
        .update_annotation(
            &id,
            input.content.as_deref().map(str::trim),
            input.page,
            input.data,
        )
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("annotation {id} not found")))?;
    Ok(Json(json!({ "annotation": annotation })))
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
    /// Parent collection, to nest this one in the taxonomy tree.
    parent_id: Option<String>,
    /// Position among siblings. Defaults to `0`; siblings then fall back to
    /// name order.
    sort_order: Option<i64>,
}

async fn create_collection(
    State(shared): State<Arc<BibShared>>,
    Json(input): Json<CreateCollectionRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<ApiError>)> {
    if input.name.trim().is_empty() {
        return Err(error(StatusCode::BAD_REQUEST, "name is required"));
    }
    let parent_id = match input.parent_id.as_deref() {
        None => None,
        // A blank value is read as "no parent" so clients can send the field
        // unconditionally instead of stripping empty strings.
        Some(raw) if raw.trim().is_empty() => None,
        Some(raw) => Some(ensure_collection_parent(&shared, None, raw).await?),
    };
    let id = format!("col-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let mut collection = Collection::new(&id, input.name.trim());
    collection.description = clean_optional(input.description);
    collection.tags = input.tags;
    collection.parent_id = parent_id;
    collection.sort_order = input.sort_order.unwrap_or(0);
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

#[derive(Deserialize)]
struct UpdateCollectionRequest {
    name: Option<String>,
    /// `null` clears the description; omitting the field leaves it alone.
    #[serde(default, deserialize_with = "double_option")]
    description: Option<Option<String>>,
    tags: Option<Vec<String>>,
    /// `null` promotes the collection back to a root; omitting the field
    /// leaves the parent untouched.
    #[serde(default, deserialize_with = "double_option")]
    parent_id: Option<Option<String>>,
    sort_order: Option<i64>,
}

async fn update_collection(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
    Json(input): Json<UpdateCollectionRequest>,
) -> ApiResult {
    let mut collection = shared
        .bib
        .get_collection(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("collection {id} not found")))?;

    if let Some(name) = &input.name {
        let name = name.trim();
        if name.is_empty() {
            return Err(error(StatusCode::BAD_REQUEST, "name cannot be empty"));
        }
        collection.name = name.to_owned();
    }
    if let Some(description) = input.description {
        collection.description = clean_optional(description);
    }
    if let Some(tags) = input.tags {
        collection.tags = tags;
    }
    if let Some(parent) = &input.parent_id {
        collection.parent_id = match parent.as_deref() {
            None => None,
            Some(raw) if raw.trim().is_empty() => None,
            Some(raw) => Some(ensure_collection_parent(&shared, Some(&id), raw).await?),
        };
    }
    if let Some(sort_order) = input.sort_order {
        collection.sort_order = sort_order;
    }
    collection.updated_at = Some(Utc::now());
    shared
        .bib
        .upsert_collection(&collection)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "collection": collection })))
}

/// Validate a `parent_id` coming from a create/update request.
///
/// `moving_id` is the collection being updated (`None` when creating). The
/// trimmed parent ID is returned on success; anything that would make the
/// collection a descendant of itself is rejected. Walking the existing chain
/// is enough: the only edge this request adds points from `moving_id` to
/// `parent_id`, so a cycle exists exactly when `parent_id`'s ancestors
/// already reach `moving_id`.
async fn ensure_collection_parent(
    shared: &BibShared,
    moving_id: Option<&str>,
    parent_id: &str,
) -> Result<String, (StatusCode, Json<ApiError>)> {
    let parent_id = parent_id.trim();
    if Some(parent_id) == moving_id {
        return Err(collection_cycle_error());
    }
    // The immediate parent has to exist: pointing a collection at nothing
    // would drop it out of every tree walk.
    let parent = shared
        .bib
        .get_collection(parent_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| {
            error(
                StatusCode::BAD_REQUEST,
                format!("parent collection {parent_id} not found"),
            )
        })?;

    let mut cursor = parent.parent_id;
    for _ in 0..MAX_COLLECTION_DEPTH {
        let Some(id) = cursor else {
            return Ok(parent_id.to_owned());
        };
        if Some(id.as_str()) == moving_id {
            return Err(collection_cycle_error());
        }
        let Some(node) = shared
            .bib
            .get_collection(&id)
            .await
            .map_err(internal)?
        else {
            // A dangling ancestor cannot loop back to `moving_id`, so the
            // chain simply ends here.
            return Ok(parent_id.to_owned());
        };
        cursor = node.parent_id;
    }
    Err(collection_cycle_error())
}

fn collection_cycle_error() -> (StatusCode, Json<ApiError>) {
    error(StatusCode::BAD_REQUEST, "collection cycle detected")
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

/// Read every persisted web-client setting.
///
/// Settings live in the shared `bib_meta` key/value table under a `web:`
/// prefix, so the TUI and the browser can share one store without either one
/// having to know the other's key names. Prefixes are stripped on the way out
/// and re-added on the way in.
async fn get_settings(State(shared): State<Arc<BibShared>>) -> ApiResult {
    let rows = shared
        .bib
        .list_meta_prefixed(SETTINGS_PREFIX)
        .await
        .map_err(internal)?;
    let mut settings = serde_json::Map::new();
    for (key, value) in rows {
        let name = key.strip_prefix(SETTINGS_PREFIX).unwrap_or(&key);
        // Values are always written back as JSON by `put_settings`; a
        // hand-edited row is skipped rather than failing the whole read.
        if let Ok(value) = serde_json::from_str::<Value>(&value) {
            settings.insert(name.to_owned(), value);
        }
    }
    Ok(Json(json!({ "settings": Value::Object(settings) })))
}

#[derive(Deserialize)]
struct PutSettingsRequest {
    settings: serde_json::Map<String, Value>,
}

/// Upsert the given settings without touching the ones left out.
///
/// A merge (rather than a replace) is deliberate: several browser tabs write
/// different slices of their state — panel layout, filters, reading
/// progress — and a full replace from one tab would silently drop the rest.
async fn put_settings(
    State(shared): State<Arc<BibShared>>,
    Json(input): Json<PutSettingsRequest>,
) -> ApiResult {
    for (key, value) in &input.settings {
        if key.is_empty() {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "setting keys must not be empty",
            ));
        }
        // The `chat:` slice of the namespace belongs to chat transcripts, so
        // a setting here can neither collide with one nor be clobbered by a
        // later `POST /chat`.
        if key.starts_with(CHAT_NAMESPACE) {
            return Err(error(
                StatusCode::BAD_REQUEST,
                format!("setting keys must not start with '{CHAT_NAMESPACE}'"),
            ));
        }
        // Serialise rather than trusting `value.to_string()`: only valid JSON
        // text can be read back by `get_settings`.
        let stored = serde_json::to_string(value).map_err(internal)?;
        shared
            .bib
            .set_meta(&format!("{SETTINGS_PREFIX}{key}"), &stored)
            .await
            .map_err(internal)?;
        // The EasyScholar key also drives the live journal-metrics client —
        // push it through so a settings save takes effect without a restart.
        // Stored as a JSON string, so only that shape is hot-applied.
        if key == "easyscholar_key" {
            let api_key = value.as_str().map(str::to_owned).filter(|k| !k.is_empty());
            shared.easyscholar.set_key(api_key);
        }
        // MinerU: `mineru_key` arms the PDF parse pipeline, `mineru_url`
        // points it at a self-hosted instance. Same hot-apply contract as
        // the EasyScholar key — takes effect without a restart.
        if key == "mineru_key" {
            let token = value.as_str().map(str::to_owned).filter(|k| !k.is_empty());
            shared.parse_hub.mineru.set_key(token);
        }
        if key == "mineru_url" {
            if let Some(url) = value.as_str().map(str::to_owned).filter(|u| !u.trim().is_empty()) {
                shared.parse_hub.mineru.set_base_url(url);
            }
        }
    }
    Ok(Json(json!({ "ok": true })))
}

/// Load the persisted `easyscholar_key` setting into the shared client.
///
/// [`put_settings`] hot-swaps only the process that receives the PUT;
/// without this read-back every fresh process would start key-less even
/// though the row is still in `bib_meta`. Call once at startup — both the
/// TUI HTTP assembly and the desktop shell do.
///
/// Precedence: a stored row always wins over `EASYSCHOLAR_KEY` (a saved key
/// is the later, more explicit intent; env only bootstraps), and an empty
/// stored string means "cleared", mirroring the PUT hot-swap — so the
/// effective key survives restarts either way. An absent row or a
/// hand-edited non-string value leaves the env-derived key untouched.
pub async fn load_stored_easyscholar_key(shared: &BibShared) {
    let Some(raw) = shared
        .bib
        .get_meta(&format!("{SETTINGS_PREFIX}easyscholar_key"))
        .await
        .ok()
        .flatten()
    else {
        return;
    };
    match serde_json::from_str::<String>(&raw) {
        // Empty mirrors the PUT hot-swap: an explicitly cleared key stays
        // cleared across restarts instead of resurrecting the env value.
        Ok(key) if key.is_empty() => shared.easyscholar.set_key(None),
        Ok(key) => shared.easyscholar.set_key(Some(key)),
        // A hand-edited non-string row is ignored rather than clobbering
        // whatever key the client already holds.
        Err(_) => {}
    }
}

// ===========================================================================
// Journal metrics (EasyScholar) — impact factor / quartiles cache
// ===========================================================================

/// Extract a non-empty trimmed journal name from an article, if any.
fn article_journal_name(article: &Article) -> Option<String> {
    article
        .journal
        .as_deref()
        .map(str::trim)
        .filter(|j| !j.is_empty())
        .map(str::to_owned)
}

/// Fire-and-forget journal-metrics enrichment for a batch of journal names.
///
/// Spawned after imports land: the response must not wait on an external
/// API, and a missing key / unknown journal / cache error must never fail
/// the caller. Names are deduped by normalized key and filtered against
/// the cache so each journal is fetched at most once per import.
fn spawn_journal_metrics_enrichment(shared: &Arc<BibShared>, journals: Vec<String>) {
    if journals.is_empty() {
        return;
    }
    let shared = Arc::clone(shared);
    tokio::spawn(async move {
        // Dedupe by normalized key, keeping the first spelling seen.
        let mut deduped: HashMap<String, String> = HashMap::new();
        for journal in journals {
            deduped
                .entry(bib_base::journal_key_of(&journal))
                .or_insert(journal);
        }
        let keys: Vec<String> = deduped.keys().cloned().collect();
        let cached = match shared.bib.get_journal_metrics(&keys).await {
            Ok(cached) => cached,
            Err(err) => {
                tracing::debug!(error = %err, "journal metrics cache read failed");
                return;
            }
        };
        for (key, journal) in deduped {
            if cached.contains_key(&key) {
                continue;
            }
            match bib_base::enrich_journal_metrics(&shared.bib, &shared.easyscholar, &journal)
                .await
            {
                Some(_) => tracing::debug!(journal = %journal, "journal metrics cached"),
                None => tracing::debug!(journal = %journal, "no journal metrics available"),
            }
        }
    });
}

/// `GET /journals/metrics` — every cached journal-metrics row.
///
/// The frontend pulls this in parallel with `/articles` and joins by
/// normalized journal name (mirroring the `/collections` membership join).
async fn list_journal_metrics(State(shared): State<Arc<BibShared>>) -> ApiResult {
    let journals = shared.bib.list_journal_metrics().await.map_err(internal)?;
    Ok(Json(json!({ "journals": journals })))
}

#[derive(Deserialize)]
struct ValidateEasyscholarQuery {
    key: Option<String>,
}

/// `GET /journals/validate-easyscholar?key=...` — probe an EasyScholar key.
///
/// Without `key` the currently configured one is validated, letting the
/// settings UI show status before the user saves anything.
async fn validate_easyscholar(
    State(shared): State<Arc<BibShared>>,
    Query(query): Query<ValidateEasyscholarQuery>,
) -> ApiResult {
    let (valid, message) = match query.key.filter(|k| !k.trim().is_empty()) {
        Some(key) => shared.easyscholar.validate(&key).await,
        None => shared.easyscholar.validate_current().await,
    };
    Ok(Json(json!({ "valid": valid, "message": message })))
}

/// `POST /articles/{id}/fetch-metrics` — blocking single-article enrichment.
///
/// Used by the paper-list context menu (获取期刊信息). Distinguished from
/// the spawned post-import enrichment by returning the fetched metrics (or
/// `null` when the journal is unknown / no key is configured) so the caller
/// can refresh the row immediately.
async fn fetch_article_metrics(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    let article = shared
        .bib
        .get_article(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("article {id} not found")))?;
    let Some(journal) = article_journal_name(&article) else {
        return Err(error(
            StatusCode::BAD_REQUEST,
            format!("article {id} has no journal name to look up"),
        ));
    };
    let metrics =
        bib_base::enrich_journal_metrics(&shared.bib, &shared.easyscholar, &journal).await;
    Ok(Json(json!({ "journal": journal, "metrics": metrics })))
}

/// Resolve and validate the chat scope from a request.
fn chat_scope(raw: Option<&str>) -> Result<String, (StatusCode, Json<ApiError>)> {
    let Some(scope) = raw.filter(|scope| !scope.is_empty()) else {
        return Err(error(StatusCode::BAD_REQUEST, "scope is required"));
    };
    // Scopes are opaque: any characters are accepted, only the key length is
    // bounded so one client cannot grow `bib_meta` keys without limit.
    if scope.len() > MAX_CHAT_SCOPE_BYTES {
        return Err(error(
            StatusCode::BAD_REQUEST,
            format!("scope must be at most {MAX_CHAT_SCOPE_BYTES} bytes"),
        ));
    }
    Ok(scope.to_owned())
}

async fn get_chat(
    State(shared): State<Arc<BibShared>>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let scope = chat_scope(params.get("scope").map(String::as_str))?;
    let stored = shared
        .bib
        .get_meta(&format!("{CHAT_PREFIX}{scope}"))
        .await
        .map_err(internal)?;
    // Unread scopes (and an unreadable leftover) are reported as `null` so a
    // fresh conversation starts from an empty transcript instead of an error.
    let payload = stored
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .unwrap_or(Value::Null);
    Ok(Json(json!({ "scope": scope, "payload": payload })))
}

#[derive(Deserialize)]
struct PostChatRequest {
    scope: String,
    payload: Value,
}

async fn post_chat(
    State(shared): State<Arc<BibShared>>,
    Json(input): Json<PostChatRequest>,
) -> ApiResult {
    let scope = chat_scope(Some(&input.scope))?;
    let stored = serde_json::to_string(&input.payload).map_err(internal)?;
    if stored.len() > MAX_CHAT_PAYLOAD_BYTES {
        return Err(error(
            StatusCode::BAD_REQUEST,
            format!("payload exceeds the {MAX_CHAT_PAYLOAD_BYTES} byte limit"),
        ));
    }
    // Whole-payload overwrite: the client always sends the transcript it
    // holds, so a merge would resurrect messages it just deleted.
    shared
        .bib
        .set_meta(&format!("{CHAT_PREFIX}{scope}"), &stored)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "ok": true, "scope": scope })))
}

/// Serve one article as CSL-JSON for citation pickers and reference
/// managers, which speak that format rather than this API's `Article`.
async fn get_article_csl_json(
    State(shared): State<Arc<BibShared>>,
    Path(id): Path<String>,
) -> ApiResult {
    let article = shared
        .bib
        .get_article(&id)
        .await
        .map_err(internal)?
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("article {id} not found")))?;
    Ok(Json(json!({ "csl_json": bib_base::to_csl_json(&article) })))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doi_scan_keeps_the_prefix_and_trims_prose() {
        assert_eq!(
            find_doi("see 10.1038/s41586-024-07666-x, for details"),
            Some("10.1038/s41586-024-07666-x".to_owned())
        );
        assert_eq!(
            find_doi("DOI: 10.9999/offline-doi-test."),
            Some("10.9999/offline-doi-test".to_owned())
        );
        // A version number is not a DOI: word boundary + digit width matter.
        assert_eq!(find_doi("version 210.12345 shipped"), None);
        assert_eq!(find_doi("see 10.99/x"), None);
    }

    #[test]
    fn arxiv_scan_accepts_common_citation_shapes() {
        assert_eq!(
            find_arxiv("arXiv:2401.12345v2 preprint"),
            Some("2401.12345v2".to_owned())
        );
        assert_eq!(
            find_arxiv("fetched from https://arxiv.org/abs/2401.12345 today"),
            Some("2401.12345".to_owned())
        );
        assert_eq!(find_arxiv("no mention at all"), None);
    }
}
