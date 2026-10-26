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
use crate::fulltext::{FullTextPage, PendingExtraction};
use crate::types::{Annotation, AnnotationKind, AnnotationRect};
use crate::zotero_model::{DeletedAttachment, FulltextItemState, NewAttachment, StoredAttachment};
use bib_types::{
    AddedBy, Article, ArticleRole, Collection, CollectionArticle, CollectionStatus, FetchStatus,
    FullText, IdKind, SearchHit, TextFormat,
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

    // Articles
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

    // Collections
    UpsertCollection {
        collection: Collection,
        reply: oneshot::Sender<Result<()>>,
    },
    GetCollection {
        id: String,
        reply: oneshot::Sender<Result<Option<Collection>>>,
    },
    ListCollections {
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
    ListArticlesNeedingExtraction {
        reply: oneshot::Sender<Result<Vec<PendingExtraction>>>,
    },
    ListExtractedForBackfill {
        reply: oneshot::Sender<Result<Vec<(String, String, TextFormat)>>>,
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

    // Annotations
    AddAnnotation {
        article_id: String,
        kind: AnnotationKind,
        content: String,
        page: Option<u32>,
        color: Option<String>,
        rects: Option<Vec<AnnotationRect>>,
        reply: oneshot::Sender<Result<Annotation>>,
    },
    ListAnnotations {
        article_id: String,
        reply: oneshot::Sender<Result<Vec<Annotation>>>,
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
            Self::UpsertCollection { collection, reply } => {
                let _ = reply.send(core.upsert_collection(&collection).await);
            }
            Self::GetCollection { id, reply } => {
                let _ = reply.send(core.get_collection(&id).await);
            }
            Self::ListCollections { status, reply } => {
                let _ = reply.send(core.list_collections(status).await);
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
            Self::ListArticlesNeedingExtraction { reply } => {
                let _ = reply.send(core.list_articles_needing_extraction().await);
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
            Self::AddAnnotation {
                article_id,
                kind,
                content,
                page,
                color,
                rects,
                reply,
            } => {
                let _ = reply.send(
                    core.add_annotation(&article_id, kind, &content, page, color, rects)
                        .await,
                );
            }
            Self::ListAnnotations { article_id, reply } => {
                let _ = reply.send(core.list_annotations(&article_id).await);
            }
            Self::DeleteAnnotation { id, reply } => {
                let _ = reply.send(core.delete_annotation(&id).await);
            }
        }
    }
}

/// Spawn the storage actor around an opened core and return the handle.
pub(crate) fn spawn(core: BibCore) -> BibBase {
    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    tokio::spawn(run(core, rx));
    BibBase { tx }
}

/// Actor body: drain commands one at a time, each strictly complete
/// before the next starts — the structural replacement for the old
/// write-gate/ungated-read split.
async fn run(core: BibCore, mut rx: mpsc::Receiver<BibCommand>) {
    while let Some(command) = rx.recv().await {
        command.run(&core).await;
    }
    tracing::debug!("bib-base storage actor exiting; database connections closed");
}

impl BibBase {
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
        self.tx
            .send(command)
            .await
            .map_err(|_| Error::ActorStopped)?;
        // A dropped reply means the actor task exited mid-operation.
        reply_rx.await.map_err(|_| Error::ActorStopped)?
    }
}
