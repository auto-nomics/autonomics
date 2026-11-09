//! Live, read-only smoke check: cargo run -p bioart --example inspect -- heart
//! Fetches one detail record and downloads one SVG (up to 8 MiB) into memory.
use bioart::BioArtClient;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = BioArtClient::new()?;
    let query = std::env::args().nth(1).unwrap_or_else(|| "heart".into());
    let page = client.search(&query, 0, 3).await?;
    println!("{}", serde_json::to_string_pretty(&page)?);
    if let Some(asset) = page.assets.first() {
        let detail = client.detail(asset.id).await?;
        println!("representations: {}", detail.representations.len());
        if let Some(file) = detail.asset.files.iter().find(|file| file.format == "SVG") {
            let download = client.download(file, 8 * 1024 * 1024).await?;
            println!(
                "downloaded {} bytes; SHA-256 {}",
                download.bytes.len(),
                download.sha256
            );
        }
    }
    Ok(())
}
