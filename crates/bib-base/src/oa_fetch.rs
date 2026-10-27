//! Best-effort PMC PDF acquisition. Original PDFs are file-backed attachments;
//! extracted text is produced later by the normal attachment extraction chain.
use crate::{BibShared, managed_fulltext, pmc_pdf};
use bib_types::{Article, FileFormat, FullText, IdKind};
use europepmc::{
    EuropePmcClient,
    types::{ResultType, SearchRequest},
};

/// Resolve PMID/DOI using Europe PMC metadata, then download a genuine PDF from
/// the current PMC Cloud Service. Missing PDFs never fall back to XML-as-TXT.
pub async fn fetch_fulltext_stored(shared: &BibShared, article: &Article) -> Option<FullText> {
    match fetch_pdf(shared, article).await {
        Ok(result) => result,
        Err(error) => {
            tracing::warn!(article_id=%article.id,%error,"PMC PDF acquisition failed; existing attachments preserved");
            None
        }
    }
}
pub(crate) async fn fetch_pdf(
    shared: &BibShared,
    article: &Article,
) -> crate::Result<Option<FullText>> {
    if shared.file_storage.is_none() {
        return Ok(None);
    }
    if let Some(ft) = shared.bib.get_fulltext(&article.id).await? {
        if ft.file_format == FileFormat::Pdf && crate::vfs_virtual_path(&ft.file_path).is_some() {
            return Ok(Some(ft));
        }
        if managed_fulltext::needs_materialization(&ft) {
            managed_fulltext::materialize_legacy(shared, ft).await?;
        }
    }
    let Some(pmcid) = resolve_pmc_id(&shared.europe_pmc, article).await? else {
        return Ok(None);
    };
    // Repeated fetches reuse a previously imported OA PDF, even when another
    // user-selected attachment remains preferred.
    for attachment in shared.bib.zotero_attachments(&article.id).await? {
        if attachment.content_type.as_deref() == Some("application/pdf")
            && attachment
                .source_url
                .as_deref()
                .is_some_and(|u| u.contains(&format!("/{pmcid}.")))
            && attachment
                .vfs_path
                .as_deref()
                .is_some_and(|u| crate::vfs_virtual_path(u).is_some())
        {
            return shared
                .bib
                .get_attachment_fulltext(&article.id, &attachment.key)
                .await;
        }
    }
    let base =
        std::env::var("ENDPOINT_PMC_CLOUD_URL").unwrap_or_else(|_| pmc_pdf::PMC_CLOUD.into());
    let Some(pdf) = pmc_pdf::download(&shared.http, &base, &pmcid).await? else {
        return Ok(None);
    };
    let ft = managed_fulltext::store_pdf(shared, &article.id, pdf).await?;
    if let Some(key) = &ft.attachment_key {
        crate::spawn_attachment_extraction(shared.clone(), article.id.clone(), key.clone());
    }
    Ok(Some(ft))
}

async fn resolve_pmc_id(
    client: &EuropePmcClient,
    article: &Article,
) -> crate::Result<Option<String>> {
    if let Some(id) = article.identifier(IdKind::Pmc) {
        return Ok(pmc_pdf::normalize_pmc_id(id));
    }
    let query = if let Some(id) = article.pmid() {
        format!("PMID:{id}")
    } else {
        let Some(doi) = article.doi() else {
            return Ok(None);
        };
        format!("DOI:{doi}")
    };
    let response = client
        .search(
            &SearchRequest::new(query)
                .result_type(ResultType::Lite)
                .page_size(10),
        )
        .await
        .map_err(|error| crate::Error::Unknown(format!("Europe PMC resolution failed: {error}")))?;
    for result in response.result_list.results {
        if let Some(pmcid) = result.pmcid.as_deref().and_then(pmc_pdf::normalize_pmc_id) {
            return Ok(Some(pmcid));
        }
        if result.source == "PMC" {
            if let Some(pmcid) = pmc_pdf::normalize_pmc_id(&result.id) {
                return Ok(Some(pmcid));
            }
        }
    }
    Ok(None)
}
