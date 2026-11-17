//! Read-only lexical catalogue search; does not download or insert icon artwork.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let query = args.next().unwrap_or_else(|| "receptor".into());
    let limit = args
        .next()
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(12);
    let author = args.next();
    let result = bioicons::BioIconsClient::new()?
        .search_with_author(&query, 0, limit, author.as_deref())
        .await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
