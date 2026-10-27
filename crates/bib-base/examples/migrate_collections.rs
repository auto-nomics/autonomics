//! Run the production migration and print aggregate collection diagnostics.
//! Usage: cargo run -p bib-base --example migrate_collections -- /path/to/bib.db
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("provide the database path explicitly")?;
    let db = bib_base::BibBase::open(&path).await?;
    let collections = db.list_collections_including_deleted(None, true).await?;
    println!(
        "{}",
        serde_json::json!({"articles":db.article_count().await?,"collections":collections.len(),"memberships":collections.iter().map(|c|c.article_ids.len()).sum::<usize>(),"roots":collections.iter().filter(|c|c.parent_id.is_none()).count(),"trashed":collections.iter().filter(|c|c.deleted).count(),"native_identities":collections.iter().all(|c|c.collection_id>0 && c.key.len()==8)})
    );
    Ok(())
}
