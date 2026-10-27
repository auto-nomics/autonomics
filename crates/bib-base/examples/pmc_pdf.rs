//! Download and validate a PMC PDF in an isolated temporary bibliography/VFS.
use std::sync::Arc;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pmcid = std::env::args().nth(1).ok_or("provide a PMCID")?;
    let storage = Arc::new(vfs::OpendalFileStorage::new_temp());
    let shared = bib_base::BibShared::open_in_memory()
        .await?
        .with_file_storage(storage.clone());
    shared
        .bib
        .upsert_article(&bib_types::Article::new("smoke", "PMC PDF validation"))
        .await?;
    let pdf = bib_base::pmc_pdf::download(&shared.http, bib_base::pmc_pdf::PMC_CLOUD, &pmcid)
        .await?
        .ok_or("PMC does not distribute a PDF for this accession")?;
    let url = pdf.source_url.clone();
    let fulltext = bib_base::managed_fulltext::store_pdf(&shared, "smoke", pdf).await?;
    let bytes = bib_base::read_full(
        &storage,
        &bib_base::vfs_virtual_path(&fulltext.file_path).unwrap(),
    )
    .await?;
    assert!(bytes.starts_with(b"%PDF-"));
    assert_eq!(Some(bytes.len() as i64), fulltext.file_size);
    println!(
        "{}",
        serde_json::json!({"pmcid":pmcid,"source_url":url,"file_format":fulltext.file_format,"file_size":fulltext.file_size,"sha256":fulltext.file_hash,"attachment_key":fulltext.attachment_key,"path":fulltext.file_path,"pdf_signature":true})
    );
    Ok(())
}
