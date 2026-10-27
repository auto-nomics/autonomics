//! File-first, attachment-owned imports and lossless repair of legacy inline OA text.
use crate::stored_files::{extracted_attachment_object_path, stored_attachment, vfs_virtual_path};
use crate::zotero_model::{self, LINK_MODE_IMPORTED_FILE, NewAttachment};
use crate::{BibBase, BibShared, Result, actor::BibCommand, bib_base::BibCore, error::Error};
use bib_types::{ExtractStatus, FileFormat, FullText, FullTextSource, TextFormat};
use turso::transaction::{Transaction, TransactionBehavior};

#[derive(Debug, Clone)]
pub(crate) enum ImportMode {
    New { expected_preferred: Option<String> },
    Repair { expected_path: String },
}
#[derive(Debug, Clone)]
pub(crate) struct ManagedImport {
    pub fulltext: FullText,
    pub filename: String,
    pub source_url: Option<String>,
    pub mode: ImportMode,
}
impl BibCore {
    pub(crate) async fn commit_managed_import(
        &self,
        input: ManagedImport,
    ) -> Result<Option<FullText>> {
        let conn = self.write_conn();
        let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate).await?;
        let result:Result<Option<FullText>>=async {
            let ft=&input.fulltext;
            let key=ft.attachment_key.as_deref().ok_or_else(||Error::Unknown("managed import requires an attachment key".into()))?;
            if self.get_article(&ft.article_id).await?.is_none(){return Err(Error::NotFound(format!("article {}",ft.article_id)));}
            let preferred=zotero_model::attachment_for_article(&tx,&ft.article_id).await?;
            let is_preferred=match &input.mode {
                ImportMode::Repair{expected_path}=>{
                    let Some(existing)=zotero_model::attachment_by_key(&tx,&ft.article_id,key).await? else{return Ok(None)};
                    if existing.vfs_path.as_deref()!=Some(expected_path){return Ok(None);}
                    tx.execute("UPDATE itemAttachments SET linkMode=?2,path=?3,vfsPath=?4,originalFilename=?5,title=CASE WHEN title IS NULL OR title=?6 THEN ?5 ELSE title END,contentType='text/plain; charset=utf-8',sourceUrl=?7,storageHash=?8,fileSize=?9 WHERE itemID=?1",turso::params![existing.item_id,LINK_MODE_IMPORTED_FILE,format!("storage:{}",input.filename),ft.file_path.clone(),input.filename.clone(),expected_path.clone(),input.source_url.clone(),ft.file_hash.clone(),ft.file_size]).await?;
                    tx.execute("INSERT INTO itemData(itemID,fieldID,valueID) VALUES (?1,?2,?3) ON CONFLICT(itemID,fieldID) DO UPDATE SET valueID=excluded.valueID",turso::params![existing.item_id,zotero_model::FIELD_TITLE,zotero_model::item_data_value_id(&tx,&input.filename).await?]).await?;
                    if let Some(url)=input.source_url.as_deref() {
                        let value=zotero_model::item_data_value_id(&tx,url).await?;
                        tx.execute("INSERT INTO itemData(itemID,fieldID,valueID) VALUES (?1,?2,?3) ON CONFLICT(itemID,fieldID) DO UPDATE SET valueID=excluded.valueID",turso::params![existing.item_id,zotero_model::FIELD_URL,value]).await?;
                    }
                    preferred.as_ref().is_some_and(|a|a.key==key)
                }
                ImportMode::New{expected_preferred}=>{
                    let current=zotero_model::LegacyFulltext::load(&tx,&ft.article_id).await?;
                    let mut rows=tx.query("SELECT source FROM fulltexts WHERE article_id=?1",turso::params![ft.article_id.clone()]).await?;
                    let oa=rows.next().await?.is_some_and(|r|r.get::<String>(0).ok().as_deref()==Some(FullTextSource::OpenAccess.as_str()));drop(rows);
                    let unchanged=preferred.as_ref().map(|a|&a.key)==expected_preferred.as_ref();
                    let choose=preferred.is_none() || (unchanged && oa && current.is_some_and(|f|f.file_format!=FileFormat::Pdf));
                    let new=NewAttachment{article_id:ft.article_id.clone(),title:Some(input.filename.clone()),filename:Some(input.filename.clone()),content_type:Some("application/pdf".into()),file_format:FileFormat::Pdf,link_mode:LINK_MODE_IMPORTED_FILE,path:Some(format!("storage:{}",input.filename)),source_url:input.source_url.clone(),vfs_path:Some(ft.file_path.clone()),storage_hash:ft.file_hash.clone(),file_size:ft.file_size};
                    zotero_model::create_attachment_with_key(&tx,&new,choose,Some(key)).await?;
                    choose
                }
            };
            let attachment=zotero_model::attachment_by_key(&tx,&ft.article_id,key).await?.ok_or_else(||Error::NotFound(format!("attachment {key}")))?;
            let extracted=ft.text_content.as_ref().map(|_|extracted_attachment_object_path(&ft.article_id,key,ft.text_format.unwrap_or(TextFormat::Plain)));
            let chars=ft.text_content.as_ref().map(|s|s.chars().count() as i64);
            tx.execute("INSERT INTO fulltextItems(itemID,version,synced,extractionStatus,extractedTextFormat,extractedBy,extractError,vfsExtractedPath,textContent,indexedChars,totalChars) VALUES (?1,1,0,?2,?3,?4,?5,?6,?7,?8,?8) ON CONFLICT(itemID) DO UPDATE SET extractionStatus=excluded.extractionStatus,extractedTextFormat=excluded.extractedTextFormat,extractedBy=excluded.extractedBy,extractError=excluded.extractError,vfsExtractedPath=excluded.vfsExtractedPath,textContent=excluded.textContent,indexedChars=excluded.indexedChars,totalChars=excluded.totalChars",turso::params![attachment.item_id,ft.extract_status.map(|s|s.as_str()),ft.text_format.map(|s|s.as_str()),ft.extracted_by.clone(),ft.extract_error.clone(),extracted,ft.text_content.clone(),chars]).await?;
            if is_preferred {Self::write_fulltext_row(&tx,ft).await?;}
            tx.execute("UPDATE collectionItems SET fetch_status='fulltext_available' WHERE itemID=(SELECT itemID FROM articleItems WHERE article_id=?1)",turso::params![ft.article_id.clone()]).await?;
            crate::bib_base::sync_search_index(&tx,&ft.article_id).await?;
            Ok(Some(ft.clone()))
        }.await;
        match result {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }
    pub(crate) async fn legacy_inline_fulltexts(&self) -> Result<Vec<FullText>> {
        let conn = self.conn();
        let mut rows=conn.query("SELECT M.article_id,I.key FROM articleItems M JOIN itemAttachments A ON A.parentItemID=M.itemID JOIN items I ON I.itemID=A.itemID JOIN fulltextItems F ON F.itemID=A.itemID WHERE F.textContent IS NOT NULL AND (A.vfsPath LIKE 'europepmc:%' OR A.sourceUrl LIKE 'europepmc:%' OR (F.extractedBy='europepmc' AND A.vfsPath LIKE '%/extracted/fulltext.txt' AND A.vfsPath NOT LIKE '%/attachments/%'))",()).await?;
        let mut ids = Vec::new();
        while let Some(row) = rows.next().await? {
            ids.push((row.get::<String>(0)?, row.get::<String>(1)?));
        }
        drop(rows);
        let mut out = Vec::new();
        for (id, key) in ids {
            if let Some(ft) = zotero_model::attachment_fulltext(&conn, &id, &key).await? {
                out.push(ft);
            }
        }
        Ok(out)
    }
}
impl BibBase {
    pub(crate) async fn commit_managed_import(
        &self,
        input: ManagedImport,
    ) -> Result<Option<FullText>> {
        self.call(|reply| BibCommand::CommitManagedImport { input, reply })
            .await
    }
    pub async fn legacy_inline_fulltexts(&self) -> Result<Vec<FullText>> {
        self.call(|reply| BibCommand::LegacyInlineFulltexts { reply })
            .await
    }
}

pub fn needs_materialization(ft: &FullText) -> bool {
    ft.file_format == FileFormat::Txt
        && (ft.file_path.starts_with("europepmc:")
            || (ft.extracted_by.as_deref() == Some("europepmc")
                && ft.file_path.ends_with("/extracted/fulltext.txt")
                && !ft.file_path.contains("/attachments/")))
}
/// Repair the existing attachment in place; its item key and annotations survive.
pub async fn materialize_legacy(shared: &BibShared, mut ft: FullText) -> Result<FullText> {
    if !needs_materialization(&ft) {
        return Ok(ft);
    }
    let storage = shared
        .file_storage
        .as_ref()
        .ok_or_else(|| Error::Unknown("bibliography VFS storage is not configured".into()))?;
    let text = ft
        .text_content
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Unknown("legacy OA attachment has no saved text".into()))?;
    let key = ft
        .attachment_key
        .clone()
        .ok_or_else(|| Error::Unknown("legacy full text has no attachment identity".into()))?;
    let old = ft.file_path.clone();
    let pmc = old
        .strip_prefix("europepmc:")
        .and_then(crate::pmc_pdf::normalize_pmc_id);
    let filename = pmc
        .as_ref()
        .map(|s| format!("{s}.txt"))
        .unwrap_or_else(|| "fulltext.txt".into());
    let stored = stored_attachment(&ft.article_id, &key, &filename, text.as_bytes());
    storage
        .write_bytes(
            &vfs_virtual_path(&stored.path).unwrap(),
            text.as_bytes().to_vec(),
        )
        .await
        .map_err(|e| Error::Unknown(e.to_string()))?;
    let extracted = extracted_attachment_object_path(&ft.article_id, &key, TextFormat::Plain);
    storage
        .write_bytes(
            &vfs_virtual_path(&extracted).unwrap(),
            text.as_bytes().to_vec(),
        )
        .await
        .map_err(|e| Error::Unknown(e.to_string()))?;
    ft.file_size = Some(text.len() as i64);
    ft.file_hash = Some(stored.file_hash);
    ft.file_path = stored.path;
    ft.source = FullTextSource::OpenAccess;
    ft.extract_status = Some(ExtractStatus::Done);
    ft.text_format = Some(TextFormat::Plain);
    ft.extract_error = None;
    let source_url =
        pmc.map(|s| format!("https://www.ebi.ac.uk/europepmc/webservices/rest/{s}/fullTextXML"));
    if let Some(result) = shared
        .bib
        .commit_managed_import(ManagedImport {
            fulltext: ft.clone(),
            filename,
            source_url,
            mode: ImportMode::Repair { expected_path: old },
        })
        .await?
    {
        return Ok(result);
    }
    shared
        .bib
        .get_attachment_fulltext(&ft.article_id, &key)
        .await?
        .ok_or_else(|| Error::NotFound(format!("attachment {key}")))
}

pub async fn repair_legacy_open_access(shared: &BibShared) -> Result<usize> {
    if shared.file_storage.is_none() {
        return Ok(0);
    }
    let rows = shared.bib.legacy_inline_fulltexts().await?;
    let mut count = 0;
    for ft in rows {
        materialize_legacy(shared, ft).await?;
        count += 1;
    }
    Ok(count)
}

pub async fn store_pdf(
    shared: &BibShared,
    article_id: &str,
    pdf: crate::pmc_pdf::PmcPdf,
) -> Result<FullText> {
    let storage = shared
        .file_storage
        .as_ref()
        .ok_or_else(|| Error::Unknown("bibliography VFS storage is not configured".into()))?;
    if !pdf.bytes.starts_with(b"%PDF-") {
        return Err(Error::Unknown("PMC download is not a PDF".into()));
    }
    let previous = shared.bib.zotero_attachment(article_id).await?;
    let key = uuid::Uuid::new_v4().simple().to_string()[..8].to_uppercase();
    let filename = pdf.filename();
    let stored = stored_attachment(article_id, &key, &filename, &pdf.bytes);
    let size = pdf.bytes.len() as i64;
    storage
        .write_bytes(&vfs_virtual_path(&stored.path).unwrap(), pdf.bytes)
        .await
        .map_err(|e| Error::Unknown(e.to_string()))?;
    let ft = FullText {
        article_id: article_id.into(),
        attachment_key: Some(key.clone()),
        file_path: stored.path,
        file_format: FileFormat::Pdf,
        text_content: None,
        source: FullTextSource::OpenAccess,
        file_hash: Some(stored.file_hash),
        file_size: Some(size),
        uploaded_at: Some(chrono::Utc::now()),
        extract_status: Some(ExtractStatus::Pending),
        text_format: None,
        extracted_by: None,
        extract_error: None,
    };
    shared
        .bib
        .commit_managed_import(ManagedImport {
            fulltext: ft,
            filename,
            source_url: Some(pdf.source_url),
            mode: ImportMode::New {
                expected_preferred: previous.map(|a| a.key),
            },
        })
        .await?
        .ok_or_else(|| Error::Unknown("PDF import did not commit".into()))
}
