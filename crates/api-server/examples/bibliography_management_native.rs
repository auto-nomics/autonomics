//! Isolated loopback host for bibliography management integration tests.
//! Uses only temporary object storage and an in-memory library.
use axum::Router;
use bib_base::BibShared;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let storage = Arc::new(vfs::OpendalFileStorage::new_temp());
    let shared = BibShared::open_in_memory()
        .await?
        .with_file_storage(storage);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    println!("BIB_MANAGEMENT_ADDR={}", listener.local_addr()?);
    let app = Router::new().nest("/api/v1/bib", api_server::bib::router(shared));
    axum::serve(listener, app).await?;
    Ok(())
}
