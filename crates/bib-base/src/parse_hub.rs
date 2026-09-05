//! Async full-text parse pipeline — the MinerU counterpart of jayread's
//! `PaperUploader` + `SseManager` pair.
//!
//! PDF uploads return as soon as the file is stored; the actual extraction
//! runs here on a background task and fans out [`ParseEvent`]s over a
//! per-article broadcast channel that the SSE endpoint subscribes to. The
//! resulting markdown lands in `fulltexts.text_content` (which
//! `sync_search_index` picks up), so a finished parse is full-text
//! searchable and readable with no further wiring.
//!
//! Restart recovery is lazy: [`ParseHub::ensure_running`] re-spawns a task
//! when a row claims `pending`/`processing` but no live task exists (the
//! in-flight MinerU job itself is lost, so the PDF is simply re-submitted).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

use crate::Result;
use crate::shared::BibShared;
use crate::stored_files::vfs_virtual_path;
use mineru::MineruClient;

/// Broadcast buffer per article. Slow SSE consumers lag past this and miss
/// intermediate progress events — the terminal `done`/`error` is what
/// matters, and the client can always re-derive state from
/// `/fulltext-statuses`.
const CHANNEL_CAPACITY: usize = 100;

/// Progress published over the article's parse stream.
#[derive(Debug, Clone, serde::Serialize)]
pub enum ParseEvent {
    /// Percent (0-100) plus a human-readable stage label.
    Progress { percent: u8, stage: String },
    /// Parse finished; markdown is stored.
    Done {
        parse_engine: String,
        markdown_length: usize,
    },
    /// Parse failed; `fulltexts.parse_error` holds the reason.
    Error { message: String },
}

impl ParseEvent {
    /// SSE event name for this variant.
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::Progress { .. } => "progress",
            Self::Done { .. } => "done",
            Self::Error { .. } => "error",
        }
    }
}

/// Orchestrates background parses: one task and one broadcast channel per
/// article, both keyed by article id.
pub struct ParseHub {
    /// MinerU client shared with the settings endpoint (hot-swappable key
    /// and base URL).
    pub mineru: Arc<MineruClient>,
    /// Articles with a live parse task — the concurrency guard that makes
    /// `spawn` idempotent and lets `reparse` answer 409.
    tasks: Mutex<HashMap<String, ()>>,
    channels: Mutex<HashMap<String, broadcast::Sender<ParseEvent>>>,
}

impl ParseHub {
    pub fn new(mineru: Arc<MineruClient>) -> Self {
        Self {
            mineru,
            tasks: Mutex::new(HashMap::new()),
            channels: Mutex::new(HashMap::new()),
        }
    }

    /// Whether a parse task is currently running for `article_id`.
    pub fn is_running(&self, article_id: &str) -> bool {
        self.tasks.lock().expect("parse task map").contains_key(article_id)
    }

    /// Subscribe to an article's parse events, creating the channel on
    /// first use. Abandoned channels (no receivers left) are pruned so the
    /// map does not grow with the library.
    pub fn subscribe(&self, article_id: &str) -> broadcast::Receiver<ParseEvent> {
        let mut channels = self.channels.lock().expect("parse channel map");
        channels.retain(|_, sender| sender.receiver_count() > 0);
        channels
            .entry(article_id.to_owned())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .subscribe()
    }

    fn publish(&self, article_id: &str, event: ParseEvent) {
        let mut channels = self.channels.lock().expect("parse channel map");
        match channels.get(article_id) {
            Some(sender) => {
                // No subscribers is fine — the DB status remains the source
                // of truth for list views.
                let _ = sender.send(event);
            }
            None => {
                // Publish without a channel (e.g. parse_stream disconnected
                // mid-flight): still create the channel so a reconnecting
                // client at least sees subsequent events.
                let (sender, _) = broadcast::channel(CHANNEL_CAPACITY);
                let _ = sender.send(event);
                channels.insert(article_id.to_owned(), sender);
            }
        }
    }

    /// Start a parse for `article_id` unless one is already running.
    ///
    /// The caller has usually just written `parse_status = 'pending'`; the
    /// task itself flips it to `processing` and on to `done`/`failed`.
    pub fn spawn(self: &Arc<Self>, shared: BibShared, article_id: String) {
        {
            let mut tasks = self.tasks.lock().expect("parse task map");
            if tasks.contains_key(&article_id) {
                return;
            }
            tasks.insert(article_id.clone(), ());
        }

        let hub = Arc::clone(self);
        tokio::spawn(async move {
            let id = article_id.as_str();
            if let Err(failure) = hub.run(&shared, id).await {
                tracing::warn!(article_id = id, error = %failure, "MinerU parse failed");
                let message = failure.to_string();
                let _ = shared.bib.set_parse_status(id, "failed", Some(&message)).await;
                hub.publish(
                    id,
                    ParseEvent::Error {
                        message: failure.to_string(),
                    },
                );
            }
            hub.tasks.lock().expect("parse task map").remove(id);
        });
    }

    /// Restart a lost task when the DB claims a parse is in flight but no
    /// task survived (fresh process, crashed worker).
    pub async fn ensure_running(self: &Arc<Self>, shared: &BibShared, article_id: &str) {
        if self.is_running(article_id) {
            return;
        }
        let pending = matches!(
            shared.bib.get_fulltext(article_id).await,
            Ok(Some(ft)) if ft.parse_status == "pending" || ft.parse_status == "processing"
        );
        if pending {
            tracing::info!(article_id, "resuming interrupted MinerU parse");
            self.spawn(shared.clone(), article_id.to_owned());
        }
    }

    /// The parse itself: VFS read → MinerU → markdown into the DB.
    async fn run(&self, shared: &BibShared, article_id: &str) -> Result<()> {
        let fulltext = shared
            .bib
            .get_fulltext(article_id)
            .await?
            .ok_or_else(|| crate::error::Error::Unknown("no full text row".into()))?;

        shared.bib.set_parse_status(article_id, "processing", None).await?;
        self.publish(
            article_id,
            ParseEvent::Progress {
                percent: 5,
                stage: "上传中".to_owned(),
            },
        );

        let bytes = read_source_bytes(shared, &fulltext.file_path).await?;
        let path = fulltext.file_path.clone();
        let size_mb = bytes.len() as f64 / 1_048_576.0;
        tracing::info!(article_id, path = %path, size_mb = format!("{size_mb:.1}"), "parsing PDF via MinerU");

        let result = self
            .mineru
            .parse_pdf(bytes, |extracted, total| {
                // Map MinerU page progress into the 10-90% band; upload
                // claimed 5%, the terminal 10% covers zip download and DB
                // write.
                let percent = if total > 0 {
                    10 + (80.0 * extracted as f64 / total as f64) as u8
                } else {
                    10
                }
                .min(90);
                self.publish(
                    article_id,
                    ParseEvent::Progress {
                        percent,
                        stage: format!("解析中 {extracted}/{total} 页"),
                    },
                );
            })
            .await?;

        let markdown = result.markdown;
        let markdown_length = markdown.chars().count();
        let mut updated = fulltext;
        updated.text_content = Some(markdown);
        updated.parse_status = "done".to_owned();
        updated.parse_engine = Some("mineru".to_owned());
        updated.parse_error = None;
        shared.bib.upsert_fulltext(&updated).await?;

        self.publish(
            article_id,
            ParseEvent::Done {
                parse_engine: "mineru".to_owned(),
                markdown_length,
            },
        );
        tracing::info!(article_id, chars = markdown_length, "MinerU parse complete");
        Ok(())
    }
}

/// Read a stored full-text file back from the VFS.
async fn read_source_bytes(shared: &BibShared, file_path: &str) -> Result<Vec<u8>> {
    let path = vfs_virtual_path(file_path).ok_or_else(|| {
        crate::error::Error::Unknown(format!("full text {file_path} is not stored in the VFS"))
    })?;
    let storage = shared.file_storage.as_ref().ok_or_else(|| {
        crate::error::Error::Unknown("bibliography VFS storage is not configured".into())
    })?;
    let size = storage.content_length(&path).await?;
    let buffer = storage.read_range(&path, 0..size).await?;
    Ok(buffer.to_vec())
}
