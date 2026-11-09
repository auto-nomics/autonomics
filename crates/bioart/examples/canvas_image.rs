//! Live PNG import check: cargo run -p bioart --example canvas_image -- cell
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = bioart::BioArtClient::new()?;
    let query = std::env::args().nth(1).unwrap_or_else(|| "cell".into());
    let result = client.search(&query, 0, 3).await?;
    for asset in result.assets {
        match client.canvas_image(asset.id).await {
            Ok(image) => {
                println!("asset={} title={:?} dimensions={}x{} sha256={} licenses={:?}", image.asset.id, image.asset.title, image.width, image.height, image.sha256, image.asset.licenses);
                return Ok(());
            }
            Err(error) => eprintln!("asset={}: {error}", asset.id),
        }
    }
    Err("no importable PNG among the first three search results".into())
}
