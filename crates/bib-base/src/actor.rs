//! Storage actor — the single owner of the bibliography's turso
//! connections.
//!
//! A turso [`turso::Connection`] clones cheaply, but every clone shares one
//! internal `ConcurrentGuard`: two statements in flight on the same
//! connection make the loser fail with `Misuse("concurrent use forbidden")`.
//! Bibliography reads historically had no gate at all, so any two
//! concurrent DAG nodes touching the library (e.g. `core/bib_save` and
//! `core/literature_fulltext` in one barrier wave) crashed one of them at
//! random (issue #143).
//!
//! Instead of patching gates around shared connections, ownership moves
//! into one spawned task using the repo's conventional actor shape (mpsc
//! command channel + oneshot replies + `tokio::spawn` worker, as in
//! `skills::evolution`): the actor owns both connections and drains
//! commands one at a time; [`BibBase`] is a cheap handle that sends a
//! [`BibCommand`] and awaits its reply. Serialization is structural —
//! exactly one task can ever touch the connections — so concurrent callers
//! (DAG nodes, TUI, agent sessions) queue in FIFO order instead of racing.
//!
//! When the last handle drops, the channel closes, the actor exits, and
//! the connections close with it.

use tokio::sync::{mpsc, oneshot};

use crate::Result;
use crate::bib_base::{BibBase, BibCore};
use crate::collections::{CollectionAddOutcome, CollectionAssignment, CollectionAssignmentOutcome};
use crate::error::Error;
use crate::fulltext::{
    ExtractedAttachmentBackfill, FullTextPage, PendingAttachmentExtraction, PendingExtraction,
};
use crate::types::{Annotation, AnnotationKind, AnnotationRect};
use crate::zotero_model::{
    AttachmentTypeSummary, DeletedAttachment, FulltextItemState, NewAttachment, StoredAttachment,
};
use bib_types::{
    AddedBy, Article, ArticleRole, Collection, CollectionArticle, CollectionStatus, FetchStatus,
    FullText, IdKind, SearchHit, SearchIndexStatus, SearchPageV2, SearchRequestV2, TextFormat,
};

/// Command channel capacity. A burst of concurrent DAG nodes issuing
/// reads and writes during a slow operation must not push commands out
/// the back door; senders beyond capacity await (backpressure) instead.
const CHANNEL_CAPACITY: usize = 128;

/// One storage operation, paired with the oneshot reply channel the
/// awaiting handle holds.
///
/// Arguments are owned (cloned at the handle boundary) so the actor never
/// borrows caller state.
pub(crate) enum BibCommand {
    // Schema
    Migrate {
        reply: oneshot::Sender<Result<()>>,
    },

    ListSavedSearches {
        reply: oneshot::Sender<Result<Vec<bib_types::SavedSearch>>>,
    },
    SaveSearch {
        id: Option<String>,
        input: bib_types::SaveSearchInput,
        reply: oneshot::Sender<Result<bib_types::SavedSearch>>,
    },
    DeleteSavedSearch {
        id: String,
        expected_version: u64,
        reply: oneshot::Sender<Result<()>>,
    },

    // Articles
    DiscoveryProvenance {
        id: String,
        offset: usize,
        limit: usize,
        reply: oneshot::Sender<Result<bib_types::discovery::DiscoveryProvenancePage>>,
    },
    PreviewDiscoveryImport {
        records: Vec<bib_types::discovery::DiscoveryImportRecord>,
        library: Option<i64>,
        collection: Option<String>,
        reply: oneshot::Sender<Result<bib_types::discovery::DiscoveryImportPreview>>,
    },
    ImportDiscovery {
        records: Vec<bib_types::discovery::DiscoveryImportRecord>,
        library: Option<i64>,
        collection: Option<String>,
        reply: oneshot::Sender<Result<Vec<bib_types::discovery::DiscoveryImportOutcome>>>,
    },
    UpsertArticle {
        article: Article,
        reply: oneshot::Sender<Result<()>>,
    },
    UpsertArticles {
        articles: Vec<Article>,
        reply: oneshot::Sender<Result<()>>,
    },
    GetArticle {
        id: String,
        reply: oneshot::Sender<Result<Option<Article>>>,
    },
    FindByIdentifier {
        kind: IdKind,
        value: String,
        reply: oneshot::Sender<Result<Option<Article>>>,
    },
    DeleteArticle {
        id: String,
        reply: oneshot::Sender<Result<u64>>,
    },
    ArticleCount {
        reply: oneshot::Sender<Result<i64>>,
    },
    ListArticleIds {
        reply: oneshot::Sender<Result<Vec<String>>>,
    },
    ListAllArticles {
        reply: oneshot::Sender<Result<Vec<Article>>>,
    },
    SearchIndexStatus {
        reply: oneshot::Sender<Result<SearchIndexStatus>>,
    },
    RequestSearchRebuild {
        reply: oneshot::Sender<Result<()>>,
    },
    RebuildSearchIndex {
        reply: oneshot::Sender<Result<()>>,
    },
    ExportSearch {
        request: SearchRequestV2,
        reply: oneshot::Sender<Result<Vec<Article>>>,
    },
    SearchEvidence {
        article_id: String,
        input: bib_types::SearchEvidenceRequest,
        reply: oneshot::Sender<Result<bib_types::SearchEvidencePage>>,
    },
    SearchV2 {
        request: SearchRequestV2,
        reply: oneshot::Sender<Result<SearchPageV2>>,
    },
    SearchArticles {
        query: String,
        limit: usize,
        offset: usize,
        reply: oneshot::Sender<Result<Vec<SearchHit>>>,
    },
    GetZoteroAttachment {
        article_id: String,
        reply: oneshot::Sender<Result<Option<StoredAttachment>>>,
    },
    GetZoteroFulltextState {
        item_id: i64,
        reply: oneshot::Sender<Result<Option<FulltextItemState>>>,
    },
    ListZoteroAttachments {
        article_id: String,
        reply: oneshot::Sender<Result<Vec<StoredAttachment>>>,
    },
    ListAttachmentTypeSummaries {
        article_ids: Vec<String>,
        reply: oneshot::Sender<Result<Vec<AttachmentTypeSummary>>>,
    },
    GetZoteroAttachmentByKey {
        article_id: String,
        key: String,
        reply: oneshot::Sender<Result<Option<StoredAttachment>>>,
    },
    CreateZoteroAttachment {
        attachment: NewAttachment,
        make_preferred: bool,
        reply: oneshot::Sender<Result<StoredAttachment>>,
    },
    SetPreferredZoteroAttachment {
        article_id: String,
        key: String,
        reply: oneshot::Sender<Result<bool>>,
    },
    SetZoteroAttachmentStorage {
        article_id: String,
        key: String,
        vfs_path: String,
        storage_hash: String,
        file_size: i64,
        reply: oneshot::Sender<Result<Option<StoredAttachment>>>,
    },
    DeleteZoteroAttachment {
        article_id: String,
        key: String,
        reply: oneshot::Sender<Result<Option<DeletedAttachment>>>,
    },

    CommitManagedImport {
        input: crate::managed_fulltext::ManagedImport,
        reply: oneshot::Sender<Result<Option<FullText>>>,
    },
    LegacyInlineFulltexts {
        reply: oneshot::Sender<Result<Vec<FullText>>>,
    },
    // Collections
    EnsureCollectionMembership {
        collection_id: String,
        article_id: String,
        reply: oneshot::Sender<Result<CollectionAddOutcome>>,
    },
    ListCollectionSummaries {
        include_deleted: bool,
        reply: oneshot::Sender<Result<Vec<serde_json::Value>>>,
    },
    QueryCollectionArticles {
        input: crate::collections::CollectionQuery,
        reply: oneshot::Sender<Result<crate::collections::CollectionPage>>,
    },
    UpsertCollection {
        collection: Collection,
        reply: oneshot::Sender<Result<()>>,
    },
    GetCollection {
        id: String,
        reply: oneshot::Sender<Result<Option<Collection>>>,
    },
    ChangeCollection {
        id: String,
        change: crate::collections::CollectionChange,
        reply: oneshot::Sender<Result<Vec<String>>>,
    },
    ListCollections {
        include_deleted: bool,
        status: Option<CollectionStatus>,
        reply: oneshot::Sender<Result<Vec<Collection>>>,
    },
    UpdateCollectionStatus {
        id: String,
        status: CollectionStatus,
        reply: oneshot::Sender<Result<()>>,
    },
    DeleteCollection {
        id: String,
        reply: oneshot::Sender<Result<()>>,
    },
    AddToCollection {
        collection_id: String,
        article_id: String,
        role: ArticleRole,
        added_by: AddedBy,
        note: Option<String>,
        reply: oneshot::Sender<Result<CollectionAddOutcome>>,
    },
    UpsertArticlesWithCollection {
        articles: Vec<Article>,
        collection_id: String,
        assignments: Vec<CollectionAssignment>,
        reply: oneshot::Sender<Result<Vec<CollectionAssignmentOutcome>>>,
    },
    RemoveFromCollection {
        collection_id: String,
        article_id: String,
        reply: oneshot::Sender<Result<()>>,
    },
    UpdateArticleRole {
        collection_id: String,
        article_id: String,
        role: ArticleRole,
        reply: oneshot::Sender<Result<()>>,
    },
    UpdateFetchStatus {
        collection_id: String,
        article_id: String,
        status: FetchStatus,
        reply: oneshot::Sender<Result<()>>,
    },
    ListCollectionArticles {
        collection_id: String,
        role_filter: Option<ArticleRole>,
        fetch_filter: Option<FetchStatus>,
        reply: oneshot::Sender<Result<Vec<CollectionArticle>>>,
    },
    ListArticlesByCollection {
        collection_id: String,
        reply: oneshot::Sender<Result<Vec<Article>>>,
    },
    ListFulltextRequests {
        collection_id: Option<String>,
        reply: oneshot::Sender<Result<Vec<CollectionArticle>>>,
    },

    // Full text
    UpsertFulltext {
        ft: FullText,
        reply: oneshot::Sender<Result<()>>,
    },
    UpsertFulltextPending {
        ft: FullText,
        reply: oneshot::Sender<Result<()>>,
    },
    RestartExtraction {
        article_id: String,
        reply: oneshot::Sender<Result<bool>>,
    },
    MarkExtractionRunning {
        article_id: String,
        reply: oneshot::Sender<Result<bool>>,
    },
    RecordExtractionSuccess {
        article_id: String,
        text: String,
        format: TextFormat,
        extracted_by: String,
        reply: oneshot::Sender<Result<()>>,
    },
    RecordExtractionFailure {
        article_id: String,
        error: String,
        reply: oneshot::Sender<Result<()>>,
    },
    ResetStaleRunning {
        reply: oneshot::Sender<Result<u64>>,
    },
    ResetStaleAttachmentRunning {
        reply: oneshot::Sender<Result<u64>>,
    },
    ListArticlesNeedingExtraction {
        reply: oneshot::Sender<Result<Vec<PendingExtraction>>>,
    },
    ListAttachmentsNeedingExtraction {
        reply: oneshot::Sender<Result<Vec<PendingAttachmentExtraction>>>,
    },
    ListExtractedForBackfill {
        reply: oneshot::Sender<Result<Vec<ExtractedAttachmentBackfill>>>,
    },
    GetFulltext {
        article_id: String,
        reply: oneshot::Sender<Result<Option<FullText>>>,
    },
    GetFulltextPage {
        article_id: String,
        offset: usize,
        limit: usize,
        reply: oneshot::Sender<Result<Option<FullTextPage>>>,
    },
    DeleteFulltext {
        article_id: String,
        reply: oneshot::Sender<Result<()>>,
    },
    HasFulltext {
        article_id: String,
        reply: oneshot::Sender<Result<bool>>,
    },
    GetAttachmentFulltext {
        article_id: String,
        attachment_key: String,
        reply: oneshot::Sender<Result<Option<FullText>>>,
    },
    RestartAttachmentExtraction {
        attachment_key: String,
        reply: oneshot::Sender<Result<Option<String>>>,
    },
    MarkAttachmentExtractionRunning {
        attachment_key: String,
        reply: oneshot::Sender<Result<bool>>,
    },
    RecordAttachmentExtractionSuccess {
        attachment_key: String,
        text: String,
        format: TextFormat,
        extracted_by: String,
        extracted_path: String,
        reply: oneshot::Sender<Result<Option<String>>>,
    },
    RecordAttachmentExtractionFailure {
        attachment_key: String,
        error: String,
        reply: oneshot::Sender<Result<Option<String>>>,
    },

    // Annotations
    AddAnnotation {
        article_id: String,
        kind: AnnotationKind,
        content: String,
        page: Option<u32>,
        color: Option<String>,
        rects: Option<Vec<AnnotationRect>>,
        attachment_key: Option<String>,
        reply: oneshot::Sender<Result<Annotation>>,
    },
    ListAnnotations {
        article_id: String,
        attachment_key: Option<String>,
        reply: oneshot::Sender<Result<Vec<Annotation>>>,
    },
    UpdateAnnotation {
        id: String,
        patch: crate::types::AnnotationUpdate,
        reply: oneshot::Sender<Result<Option<Annotation>>>,
    },
    DeleteAnnotation {
        id: String,
        reply: oneshot::Sender<Result<()>>,
    },
}

impl BibCommand {
    /// Execute against the actor-owned core and deliver the reply.
    ///
    /// A failed `send` only means the caller went away (cancelled /
    /// timed out) — nothing to act on.
    async fn run(self, core: &BibCore) {
        match self {
            Self::Migrate { reply } => {
                let _ = reply.send(core.migrate().await);
            }
            Self::UpsertArticle { article, reply } => {
                let _ = reply.send(core.upsert_article(&article).await);
            }
            Self::DiscoveryProvenance {
                id,
                offset,
                limit,
                reply,
            } => {
                let _ = reply.send(core.discovery_provenance(&id, offset, limit).await);
            }
            Self::PreviewDiscoveryImport {
                records,
                library,
                collection,
                reply,
            } => {
                let _ = reply.send(
                    core.preview_discovery_import(records, library, collection)
                        .await,
                );
            }
            Self::ImportDiscovery {
                records,
                library,
                collection,
                reply,
            } => {
                let _ = reply.send(core.import_discovery(records, library, collection).await);
            }
            Self::UpsertArticles { articles, reply } => {
                let _ = reply.send(core.upsert_articles(&articles).await);
            }
            Self::GetArticle { id, reply } => {
                let _ = reply.send(core.get_article(&id).await);
            }
            Self::FindByIdentifier { kind, value, reply } => {
                let _ = reply.send(core.find_by_identifier(kind, &value).await);
            }
            Self::DeleteArticle { id, reply } => {
                let _ = reply.send(core.delete_article(&id).await);
            }
            Self::ArticleCount { reply } => {
                let _ = reply.send(core.article_count().await);
            }
            Self::ListArticleIds { reply } => {
                let _ = reply.send(core.list_article_ids().await);
            }
            Self::ListAllArticles { reply } => {
                let _ = reply.send(core.list_all_articles().await);
            }
            Self::ListSavedSearches { reply } => {
                let _ = reply.send(core.list_saved_searches().await);
            }
            Self::SaveSearch { id, input, reply } => {
                let _ = reply.send(core.save_search(id.as_deref(), input).await);
            }
            Self::DeleteSavedSearch {
                id,
                expected_version,
                reply,
            } => {
                let _ = reply.send(core.delete_saved_search(&id, expected_version).await);
            }
            Self::SearchIndexStatus { reply } => {
                let _ = reply.send(core.search_index_status().await);
            }
            Self::RequestSearchRebuild { reply } => {
                let _ = reply.send(Err(Error::InvalidSearchQuery(
                    "background indexing is disabled by the legacy engine rollout flag".into(),
                )));
            }
            Self::RebuildSearchIndex { reply } => {
                if core.search_engine == crate::SearchEngineMode::Legacy {
                    let _ = reply.send(Err(Error::InvalidSearchQuery(
                        "index rebuilding is disabled by the legacy engine rollout flag".into(),
                    )));
                    return;
                }
                if let Ok(mut index) = core.search_index.lock() {
                    index.revision = None;
                }
                let _ = reply.send(core.refresh_search_index().await);
            }
            Self::ExportSearch { request, reply } => {
                let _ = reply.send(core.export_search(request).await);
            }
            Self::SearchEvidence {
                article_id,
                input,
                reply,
            } => {
                let _ = reply.send(core.search_evidence(&article_id, input).await);
            }
            Self::SearchV2 { request, reply } => {
                let _ = reply.send(core.search_v2(request).await);
            }
            Self::SearchArticles {
                query,
                limit,
                offset,
                reply,
            } => {
                let _ = reply.send(core.search_articles(&query, limit, offset).await);
            }
            Self::GetZoteroAttachment { article_id, reply } => {
                let _ = reply.send(
                    crate::zotero_model::attachment_for_article(&core.conn(), &article_id).await,
                );
            }
            Self::GetZoteroFulltextState { item_id, reply } => {
                let _ =
                    reply.send(crate::zotero_model::fulltext_state(&core.conn(), item_id).await);
            }
            Self::ListZoteroAttachments { article_id, reply } => {
                let _ = reply.send(
                    crate::zotero_model::list_attachments_for_article(&core.conn(), &article_id)
                        .await,
                );
            }
            Self::ListAttachmentTypeSummaries { article_ids, reply } => {
                let _ = reply.send(
                    crate::zotero_model::attachment_type_summaries(&core.conn(), &article_ids)
                        .await,
                );
            }
            Self::GetZoteroAttachmentByKey {
                article_id,
                key,
                reply,
            } => {
                let _ = reply.send(
                    crate::zotero_model::attachment_by_key(&core.conn(), &article_id, &key).await,
                );
            }
            Self::CreateZoteroAttachment {
                attachment,
                make_preferred,
                reply,
            } => {
                let _ = reply.send(
                    crate::zotero_model::create_attachment(
                        &core.conn(),
                        &attachment,
                        make_preferred,
                    )
                    .await,
                );
            }
            Self::SetPreferredZoteroAttachment {
                article_id,
                key,
                reply,
            } => {
                let _ = reply.send(
                    crate::zotero_model::set_preferred_attachment(&core.conn(), &article_id, &key)
                        .await,
                );
            }
            Self::SetZoteroAttachmentStorage {
                article_id,
                key,
                vfs_path,
                storage_hash,
                file_size,
                reply,
            } => {
                let _ = reply.send(
                    crate::zotero_model::set_attachment_storage(
                        &core.conn(),
                        &article_id,
                        &key,
                        &vfs_path,
                        &storage_hash,
                        file_size,
                    )
                    .await,
                );
            }
            Self::DeleteZoteroAttachment {
                article_id,
                key,
                reply,
            } => {
                let _ = reply.send(
                    crate::zotero_model::delete_attachment_by_key(&core.conn(), &article_id, &key)
                        .await,
                );
            }
            Self::EnsureCollectionMembership {
                collection_id,
                article_id,
                reply,
            } => {
                let _ = reply.send(
                    core.ensure_collection_membership(&collection_id, &article_id)
                        .await,
                );
            }
            Self::ListCollectionSummaries {
                include_deleted,
                reply,
            } => {
                let _ = reply.send(core.list_collection_summaries(include_deleted).await);
            }
            Self::QueryCollectionArticles { input, reply } => {
                let _ = reply.send(core.query_collection_articles(input).await);
            }
            Self::UpsertCollection { collection, reply } => {
                let _ = reply.send(core.upsert_collection(&collection).await);
            }
            Self::GetCollection { id, reply } => {
                let _ = reply.send(core.get_collection(&id).await);
            }
            Self::ChangeCollection { id, change, reply } => {
                let _ = reply.send(core.change_collection(&id, change).await);
            }
            Self::ListCollections {
                status,
                include_deleted,
                reply,
            } => {
                let _ = reply.send(
                    core.list_collections_including_deleted(status, include_deleted)
                        .await,
                );
            }
            Self::UpdateCollectionStatus { id, status, reply } => {
                let _ = reply.send(core.update_collection_status(&id, status).await);
            }
            Self::DeleteCollection { id, reply } => {
                let _ = reply.send(core.delete_collection(&id).await);
            }
            Self::AddToCollection {
                collection_id,
                article_id,
                role,
                added_by,
                note,
                reply,
            } => {
                let _ = reply.send(
                    core.add_to_collection(
                        &collection_id,
                        &article_id,
                        role,
                        added_by,
                        note.as_deref(),
                    )
                    .await,
                );
            }
            Self::UpsertArticlesWithCollection {
                articles,
                collection_id,
                assignments,
                reply,
            } => {
                let _ = reply.send(
                    core.upsert_articles_with_collection(&articles, &collection_id, &assignments)
                        .await,
                );
            }
            Self::RemoveFromCollection {
                collection_id,
                article_id,
                reply,
            } => {
                let _ = reply.send(
                    core.remove_from_collection(&collection_id, &article_id)
                        .await,
                );
            }
            Self::UpdateArticleRole {
                collection_id,
                article_id,
                role,
                reply,
            } => {
                let _ = reply.send(
                    core.update_article_role(&collection_id, &article_id, role)
                        .await,
                );
            }
            Self::UpdateFetchStatus {
                collection_id,
                article_id,
                status,
                reply,
            } => {
                let _ = reply.send(
                    core.update_fetch_status(&collection_id, &article_id, status)
                        .await,
                );
            }
            Self::ListCollectionArticles {
                collection_id,
                role_filter,
                fetch_filter,
                reply,
            } => {
                let _ = reply.send(
                    core.list_collection_articles(&collection_id, role_filter, fetch_filter)
                        .await,
                );
            }
            Self::ListArticlesByCollection {
                collection_id,
                reply,
            } => {
                let _ = reply.send(core.list_articles_by_collection(&collection_id).await);
            }
            Self::ListFulltextRequests {
                collection_id,
                reply,
            } => {
                let _ = reply.send(core.list_fulltext_requests(collection_id.as_deref()).await);
            }
            Self::CommitManagedImport { input, reply } => {
                let _ = reply.send(core.commit_managed_import(input).await);
            }
            Self::LegacyInlineFulltexts { reply } => {
                let _ = reply.send(core.legacy_inline_fulltexts().await);
            }
            Self::UpsertFulltext { ft, reply } => {
                let _ = reply.send(core.upsert_fulltext(&ft).await);
            }
            Self::UpsertFulltextPending { ft, reply } => {
                let _ = reply.send(core.upsert_fulltext_pending(&ft).await);
            }
            Self::RestartExtraction { article_id, reply } => {
                let _ = reply.send(core.restart_extraction(&article_id).await);
            }
            Self::MarkExtractionRunning { article_id, reply } => {
                let _ = reply.send(core.mark_extraction_running(&article_id).await);
            }
            Self::RecordExtractionSuccess {
                article_id,
                text,
                format,
                extracted_by,
                reply,
            } => {
                let _ = reply.send(
                    core.record_extraction_success(&article_id, &text, format, &extracted_by)
                        .await,
                );
            }
            Self::RecordExtractionFailure {
                article_id,
                error,
                reply,
            } => {
                let _ = reply.send(core.record_extraction_failure(&article_id, &error).await);
            }
            Self::ResetStaleRunning { reply } => {
                let _ = reply.send(core.reset_stale_running().await);
            }
            Self::ResetStaleAttachmentRunning { reply } => {
                let _ = reply.send(core.reset_stale_attachment_running().await);
            }
            Self::ListArticlesNeedingExtraction { reply } => {
                let _ = reply.send(core.list_articles_needing_extraction().await);
            }
            Self::ListAttachmentsNeedingExtraction { reply } => {
                let _ = reply.send(core.list_attachments_needing_extraction().await);
            }
            Self::ListExtractedForBackfill { reply } => {
                let _ = reply.send(core.list_extracted_for_backfill().await);
            }
            Self::GetFulltext { article_id, reply } => {
                let _ = reply.send(core.get_fulltext(&article_id).await);
            }
            Self::GetFulltextPage {
                article_id,
                offset,
                limit,
                reply,
            } => {
                let _ = reply.send(core.get_fulltext_page(&article_id, offset, limit).await);
            }
            Self::DeleteFulltext { article_id, reply } => {
                let _ = reply.send(core.delete_fulltext(&article_id).await);
            }
            Self::HasFulltext { article_id, reply } => {
                let _ = reply.send(core.has_fulltext(&article_id).await);
            }
            Self::GetAttachmentFulltext {
                article_id,
                attachment_key,
                reply,
            } => {
                let _ = reply.send(
                    core.get_attachment_fulltext(&article_id, &attachment_key)
                        .await,
                );
            }
            Self::RestartAttachmentExtraction {
                attachment_key,
                reply,
            } => {
                let _ = reply.send(core.restart_attachment_extraction(&attachment_key).await);
            }
            Self::MarkAttachmentExtractionRunning {
                attachment_key,
                reply,
            } => {
                let _ = reply.send(
                    core.mark_attachment_extraction_running(&attachment_key)
                        .await,
                );
            }
            Self::RecordAttachmentExtractionSuccess {
                attachment_key,
                text,
                format,
                extracted_by,
                extracted_path,
                reply,
            } => {
                let _ = reply.send(
                    core.record_attachment_extraction_success(
                        &attachment_key,
                        &text,
                        format,
                        &extracted_by,
                        &extracted_path,
                    )
                    .await,
                );
            }
            Self::RecordAttachmentExtractionFailure {
                attachment_key,
                error,
                reply,
            } => {
                let _ = reply.send(
                    core.record_attachment_extraction_failure(&attachment_key, &error)
                        .await,
                );
            }
            Self::AddAnnotation {
                article_id,
                kind,
                content,
                page,
                color,
                rects,
                attachment_key,
                reply,
            } => {
                let _ = reply.send(
                    core.add_annotation(
                        &article_id,
                        kind,
                        &content,
                        page,
                        color,
                        rects,
                        attachment_key.as_deref(),
                    )
                    .await,
                );
            }
            Self::ListAnnotations {
                article_id,
                attachment_key,
                reply,
            } => {
                let _ = reply.send(
                    core.list_annotations(&article_id, attachment_key.as_deref())
                        .await,
                );
            }
            Self::UpdateAnnotation { id, patch, reply } => {
                let _ = reply.send(core.update_annotation(&id, patch).await);
            }
            Self::DeleteAnnotation { id, reply } => {
                let _ = reply.send(core.delete_annotation(&id).await);
            }
        }
    }
}

/// Spawn the storage actor around an opened core and return the handle.
pub(crate) fn spawn(mut core: BibCore) -> BibBase {
    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    let search_tx = core.search_context.take().map(|context| {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        tokio::spawn(crate::search::run_search_service(*context, rx));
        tx
    });
    tokio::spawn(run(core, rx, search_tx));
    let (changes, _) = tokio::sync::watch::channel(chrono::Utc::now().timestamp_millis() as u64);
    BibBase { tx, changes }
}

enum SearchRoute {
    Search(crate::search::SearchJob),
    Storage(BibCommand),
}

impl BibCommand {
    fn into_search_job(self) -> SearchRoute {
        use crate::search::SearchJob;
        let job = match self {
            Self::ExportSearch { request, reply } => SearchJob::Export { request, reply },
            Self::SearchEvidence {
                article_id,
                input,
                reply,
            } => SearchJob::Evidence {
                article_id,
                input,
                reply,
            },
            Self::SearchV2 { request, reply } => SearchJob::V2 { request, reply },
            Self::SearchArticles {
                query,
                limit,
                offset,
                reply,
            } => SearchJob::Legacy {
                query,
                limit,
                offset,
                reply,
            },
            Self::QueryCollectionArticles { input, reply } if !input.query.trim().is_empty() => {
                SearchJob::Collection { input, reply }
            }
            Self::RebuildSearchIndex { reply } => SearchJob::Rebuild { reply: Some(reply) },
            other => return SearchRoute::Storage(other),
        };
        SearchRoute::Search(job)
    }
}

/// Each actor owns its connection. Searches/replays use a dedicated read actor;
/// mutations and small reads retain the existing FIFO storage actor.
async fn run(
    core: BibCore,
    mut rx: mpsc::Receiver<BibCommand>,
    search_tx: Option<mpsc::Sender<crate::search::SearchJob>>,
) {
    while let Some(command) = rx.recv().await {
        if let Some(search_tx) = &search_tx {
            if let BibCommand::RequestSearchRebuild { reply } = command {
                core.mark_build_requested();
                let result = search_tx
                    .send(crate::search::SearchJob::Rebuild { reply: None })
                    .await
                    .map_err(|_| Error::ActorStopped);
                let _ = reply.send(result);
                continue;
            }
            match command.into_search_job() {
                SearchRoute::Search(job) => {
                    if matches!(&job, crate::search::SearchJob::Rebuild { .. }) {
                        core.mark_build_requested();
                    }
                    if let Err(error) = search_tx.send(job).await {
                        error.0.reject();
                    }
                }
                SearchRoute::Storage(command) => command.run(&core).await,
            }
        } else {
            command.run(&core).await;
        }
    }
    tracing::debug!("bib-base storage actor exiting; database connections closed");
}

impl BibBase {
    /// Subscribers are notified only after a successful committed operation.
    pub fn subscribe_collection_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// Send one command to the storage actor and await its reply.
    ///
    /// `make` receives the reply channel so the command carries it
    /// without the caller juggling oneshot types.
    pub(crate) async fn call<T, F>(&self, make: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(oneshot::Sender<Result<T>>) -> BibCommand,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        let command = make(reply_tx);
        let modifies_collections = matches!(
            &command,
            BibCommand::UpsertArticle { .. }
                | BibCommand::UpsertArticles { .. }
                | BibCommand::DeleteArticle { .. }
                | BibCommand::CommitManagedImport { .. }
                | BibCommand::UpsertCollection { .. }
                | BibCommand::ChangeCollection { .. }
                | BibCommand::DeleteCollection { .. }
                | BibCommand::EnsureCollectionMembership { .. }
                | BibCommand::AddToCollection { .. }
                | BibCommand::RemoveFromCollection { .. }
                | BibCommand::UpsertArticlesWithCollection { .. }
                | BibCommand::UpdateArticleRole { .. }
                | BibCommand::UpdateFetchStatus { .. }
                | BibCommand::UpdateCollectionStatus { .. }
        );
        self.tx
            .send(command)
            .await
            .map_err(|_| Error::ActorStopped)?;
        // A dropped reply means the actor task exited mid-operation.
        let result = reply_rx.await.map_err(|_| Error::ActorStopped)?;
        if modifies_collections && result.is_ok() {
            self.changes.send_modify(|version| *version += 1);
        }
        result
    }
}
