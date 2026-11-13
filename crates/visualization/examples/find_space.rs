//! Read-only placement audit of an explicit saved scene; no store or figure writes.
use std::io::Read;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 5 {
        return Err("usage: find_space SCENE_JSON REFERENCE_ID WIDTH HEIGHT GAP".into());
    }
    let limit = 64 * 1024 * 1024;
    let mut bytes = Vec::new();
    std::fs::File::open(&args[0])?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("scene JSON exceeds 64 MiB".into());
    }
    let input: serde_json::Value = serde_json::from_slice(&bytes)?;
    let document = input
        .pointer("/scene/document")
        .or_else(|| input.get("document"))
        .unwrap_or(&input);
    let spec = visualization::placement::Spec {
        relative_to: args[1].clone(),
        width: args[2].parse()?,
        height: args[3].parse()?,
        gap: args[4].parse()?,
    };
    let result = visualization::placement::find(document, &spec)?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
