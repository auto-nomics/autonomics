//! Read-only local SVG path diagnostics; does not contact a catalogue or modify artwork.
use std::io::Read;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("provide a local SVG path")?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("SVG exceeds 4 MiB".into());
    }
    let started = std::time::Instant::now();
    let mut value = serde_json::to_value(bioicons::parts::inspect(&bytes)?)?;
    let object = value.as_object_mut().ok_or("invalid inspection")?;
    object.remove("previewDataURL");
    object.remove("annotatedDataURL");
    object.insert(
        "inspection_elapsed_ms".into(),
        serde_json::json!(started.elapsed().as_millis()),
    );
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
