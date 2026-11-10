//! Opt-in live catalogue/download check. Does not modify projects.
use base64::{Engine, engine::general_purpose::STANDARD};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let query = args.next().unwrap_or_else(|| "receptor".into());
    let client = bioicons::BioIconsClient::new()?;
    let page = client.search(&query, 0, 3).await?;
    println!("{}", serde_json::to_string_pretty(&page)?);
    if let Some(asset) = page.assets.first() {
        let vector = client.vector(&asset.id).await?;
        println!(
            "normalized {} {}x{} source={} vector={} colors={:?}",
            asset.name,
            vector.vector.width,
            vector.vector.height,
            vector.vector.source_sha256,
            vector.vector.sha256,
            vector.vector.colors
        );
        if let Some(prefix) = args.next() {
            for (suffix, url) in [
                ("svg", vector.vector.data_url),
                ("png", vector.vector.preview_data_url),
            ] {
                let path = format!("{prefix}.{suffix}");
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?;
                use std::io::Write;
                file.write_all(&STANDARD.decode(url.split_once(',').unwrap().1)?)?;
            }
        }
    }
    Ok(())
}
