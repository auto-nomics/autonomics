//! Copy a VFS file into an existing literature record's managed attachments.
use std::sync::Arc;

use agentik_core::tools::{ToolError, ToolFunction};
use agentik_proc::tool;
use agentik_sdk::types::ToolResult;
use async_trait::async_trait;
use bib_types::{ExtractStatus, FileFormat, FullText, FullTextSource};

use crate::{
    BibShared, Error,
    managed_fulltext::{ImportMode, ManagedImport},
};

const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[tool(
    name = "literature_attach",
    description = "Add a file attachment to an existing literature-library record. Copies a VFS file (up to 64 MiB) into library-managed storage; use the article_id returned by literature_import or bib_search_library. Accepts canonical domain://path URIs or VFS paths, not HTTP URLs or host filesystem paths. Preserves existing attachments. The first attachment becomes preferred; otherwise preferred defaults to false. PDF, HTML and TXT files are extracted in the background; other files are stored without extraction. Returns the attachment key and managed file path."
)]
pub struct LiteratureAttachInput {
    #[desc = "Existing library article id"]
    pub article_id: String,
    #[desc = "Source file URI/path in VFS, e.g. local://browser-downloads/paper.pdf"]
    pub file_path: String,
    #[desc = "Optional attachment filename; defaults to the source basename"]
    pub filename: Option<String>,
    #[desc = "Optional attachment display title"]
    pub title: Option<String>,
    #[desc = "Make this the preferred attachment (default false; the first attachment is always preferred)"]
    pub preferred: Option<bool>,
}

pub struct LiteratureAttachTool {
    pub shared: Arc<BibShared>,
}

#[async_trait]
impl ToolFunction for LiteratureAttachTool {
    type Input = LiteratureAttachInput;

    async fn run(&self, input: Self::Input) -> Result<ToolResult, ToolError> {
        self.attach(input)
            .await
            .map(ToolResult::success_json)
            .map_err(|error| ToolError::ExecutionFailed {
                source: error.to_string().into(),
            })
    }
}

impl LiteratureAttachTool {
    async fn attach(&self, input: LiteratureAttachInput) -> crate::Result<serde_json::Value> {
        if input.article_id.trim().is_empty() || input.file_path.trim().is_empty() {
            return Err(Error::Unknown(
                "article_id and file_path must not be empty".into(),
            ));
        }
        if input.file_path.starts_with("http://") || input.file_path.starts_with("https://") {
            return Err(Error::Unknown(
                "download the file to VFS before attaching it".into(),
            ));
        }
        let storage =
            self.shared.file_storage.as_ref().ok_or_else(|| {
                Error::Unknown("bibliography VFS storage is not configured".into())
            })?;
        if self
            .shared
            .bib
            .get_article(&input.article_id)
            .await?
            .is_none()
        {
            return Err(Error::NotFound(format!(
                "article {} (missing or in trash)",
                input.article_id
            )));
        }
        let filename = input.filename.unwrap_or_else(|| {
            input
                .file_path
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned()
        });
        if filename.trim().is_empty()
            || filename == "."
            || filename == ".."
            || filename.contains(['/', '\\'])
        {
            return Err(Error::Unknown(
                "filename must be a non-empty basename".into(),
            ));
        }
        let io_error = |error: vfs::opendal::Error| Error::Unknown(error.to_string());
        let length = storage
            .content_length(&input.file_path)
            .await
            .map_err(io_error)?;
        if length == 0 || length > MAX_BYTES {
            return Err(Error::Unknown(
                "attachment must contain 1 byte to 64 MiB".into(),
            ));
        }
        // Bound the read as well: the source can change after the size check.
        let bytes = storage
            .read_range(&input.file_path, 0..length)
            .await
            .map_err(io_error)?
            .to_vec();
        if bytes.len() as u64 > MAX_BYTES || bytes.is_empty() {
            return Err(Error::Unknown(
                "attachment must contain 1 byte to 64 MiB".into(),
            ));
        }
        let extension = filename
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
            .unwrap_or_default();
        let (format, content_type, extract) = match extension.as_str() {
            "pdf" => (FileFormat::Pdf, "application/pdf", true),
            "html" | "htm" => (FileFormat::Html, "text/html", true),
            "txt" => (FileFormat::Txt, "text/plain", true),
            _ => (FileFormat::Txt, "application/octet-stream", false),
        };
        if format == FileFormat::Pdf && !bytes.starts_with(b"%PDF-") {
            return Err(Error::Unknown(
                "file has a .pdf extension but is not a PDF".into(),
            ));
        }
        let key = uuid::Uuid::new_v4().simple().to_string()[..8].to_uppercase();
        let stored = crate::stored_attachment(&input.article_id, &key, &filename, &bytes)
            .canonicalized(storage)
            .map_err(io_error)?;
        let path = crate::vfs_virtual_path(&stored.path)
            .ok_or_else(|| Error::Unknown("invalid managed storage URI".into()))?;
        let fulltext = FullText {
            article_id: input.article_id.clone(),
            attachment_key: Some(key.clone()),
            file_path: stored.path.clone(),
            file_format: format,
            text_content: None,
            source: FullTextSource::UserUpload,
            file_hash: Some(stored.file_hash),
            file_size: Some(bytes.len() as i64),
            uploaded_at: Some(chrono::Utc::now()),
            extract_status: extract.then_some(ExtractStatus::Pending),
            text_format: None,
            extracted_by: None,
            extract_error: None,
        };
        storage.write_bytes(&path, bytes).await.map_err(io_error)?;
        let result = self
            .shared
            .bib
            .commit_managed_import(ManagedImport {
                fulltext,
                filename,
                source_url: None,
                mode: ImportMode::Attach {
                    title: input.title,
                    content_type: content_type.into(),
                    make_preferred: input.preferred.unwrap_or(false),
                },
            })
            .await;
        match result {
            Ok(Some(_)) => {}
            other => {
                let _ = storage.delete_object(&path).await;
                other?;
                return Err(Error::Unknown("attachment import did not commit".into()));
            }
        }
        if extract {
            crate::spawn_attachment_extraction(
                self.shared.as_ref().clone(),
                input.article_id.clone(),
                key.clone(),
            );
        }
        Ok(
            serde_json::json!({"article_id": input.article_id, "attachment_key": key, "file_path": stored.path, "extraction_queued": extract}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn tool() -> LiteratureAttachTool {
        let mut shared = BibShared::open_in_memory().await.unwrap();
        shared.file_storage = Some(Arc::new(vfs::OpendalFileStorage::new_temp()));
        shared
            .bib
            .upsert_article(&bib_types::Article::new("paper", "Paper"))
            .await
            .unwrap();
        shared
            .file_storage
            .as_ref()
            .unwrap()
            .write_bytes("/source.bin", vec![0, 1, 2, 255])
            .await
            .unwrap();
        LiteratureAttachTool {
            shared: Arc::new(shared),
        }
    }

    fn input() -> LiteratureAttachInput {
        LiteratureAttachInput {
            article_id: "paper".into(),
            file_path: "/source.bin".into(),
            filename: None,
            title: Some("Supplement".into()),
            preferred: None,
        }
    }

    #[tokio::test]
    async fn copies_bytes_and_preserves_preferred_until_explicitly_changed() {
        let tool = tool().await;
        let first = tool.attach(input()).await.unwrap();
        let first_key = first["attachment_key"].as_str().unwrap();
        let storage = tool.shared.file_storage.as_ref().unwrap();
        assert_eq!(
            storage
                .read_bytes(first["file_path"].as_str().unwrap())
                .await
                .unwrap(),
            vec![0, 1, 2, 255]
        );
        assert_eq!(
            storage.read_bytes("/source.bin").await.unwrap(),
            vec![0, 1, 2, 255]
        );
        assert_eq!(first["extraction_queued"], false);
        let second = tool.attach(input()).await.unwrap();
        assert_ne!(first["attachment_key"], second["attachment_key"]);
        assert_eq!(
            tool.shared
                .bib
                .zotero_attachment("paper")
                .await
                .unwrap()
                .unwrap()
                .key,
            first_key
        );
        let mut preferred = input();
        preferred.preferred = Some(true);
        let third = tool.attach(preferred).await.unwrap();
        assert_eq!(
            tool.shared
                .bib
                .zotero_attachment("paper")
                .await
                .unwrap()
                .unwrap()
                .key,
            third["attachment_key"].as_str().unwrap()
        );
        let attachments = tool.shared.bib.zotero_attachments("paper").await.unwrap();
        assert_eq!(attachments.len(), 3);
        assert!(
            attachments
                .iter()
                .all(|a| a.title.as_deref() == Some("Supplement") && a.storage_hash.is_some())
        );
    }

    #[tokio::test]
    async fn rejects_missing_records_missing_files_and_invalid_pdf() {
        let tool = tool().await;
        let mut missing = input();
        missing.article_id = "missing".into();
        assert!(tool.attach(missing).await.is_err());
        let mut missing = input();
        missing.file_path = "/missing.bin".into();
        assert!(tool.attach(missing).await.is_err());
        let mut invalid = input();
        invalid.filename = Some("paper.pdf".into());
        assert!(tool.attach(invalid).await.is_err());
        let mut invalid = input();
        invalid.filename = Some("../paper.bin".into());
        assert!(tool.attach(invalid).await.is_err());
        assert!(
            tool.shared
                .bib
                .zotero_attachments("paper")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn rejects_trashed_records_without_adding_attachments() {
        let tool = tool().await;
        tool.shared
            .bib
            .set_article_trash("paper", true)
            .await
            .unwrap();
        assert!(tool.attach(input()).await.is_err());
        assert!(
            tool.shared
                .bib
                .zotero_attachments("paper")
                .await
                .unwrap()
                .is_empty()
        );
    }
}
