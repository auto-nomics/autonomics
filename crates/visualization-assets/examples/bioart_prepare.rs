//! Opt-in read-only BioArt SVG preparation and preview, without project writes.
use base64::{Engine, engine::general_purpose::STANDARD};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let id = args
        .next()
        .ok_or("provide a discovered numeric BioArt asset ID")?
        .parse()?;
    let asset = visualization_assets::bioart::prepare(
        &bioart::BioArtClient::new()?,
        id,
        visualization_assets::bioart::Format::Svg,
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&asset.metadata())?);
    if let Some(prefix) = args.next() {
        for (suffix, url) in [
            ("svg", asset.data_url.as_str()),
            ("png", asset.preview_url()),
        ] {
            let path = format!("{prefix}.{suffix}");
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
            use std::io::Write;
            file.write_all(&STANDARD.decode(url.split_once(',').ok_or("invalid data URL")?.1)?)?;
        }
    }
    Ok(())
}
