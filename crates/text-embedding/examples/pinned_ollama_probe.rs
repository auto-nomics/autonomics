//! Validate an installed model revision with real English/Chinese inputs.
//! cargo run -p text-embedding --example pinned_ollama_probe -- MODEL SHA256 DIMENSIONS
use std::time::Duration;
use text_embedding::{
    EmbeddingInput, OllamaInferenceOptions, PinnedOllamaEmbedder, TextEmbedder, cosine,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !matches!(args.len(), 3 | 4) {
        return Err(
            "usage: pinned_ollama_probe MODEL SHA256 DIMENSIONS [INFERENCE_JSON] (loopback only)"
                .into(),
        );
    }
    let inference: OllamaInferenceOptions = args
        .get(3)
        .map(|v| serde_json::from_str(v))
        .transpose()?
        .unwrap_or_default();
    let provider = PinnedOllamaEmbedder::connect_with_inference(
        "http://127.0.0.1:11434",
        &args[0],
        &args[1],
        args[2].parse()?,
        16,
        Duration::from_secs(60),
        inference.clone(),
    )
    .await?;
    let vectors = provider
        .batch_embed(&[
            EmbeddingInput::new("en", "gene expression in immune cells"),
            EmbeddingInput::new("zh", "免疫细胞中的基因表达"),
        ])
        .await?;
    println!(
        "{}",
        serde_json::json!({"signature":provider.signature(), "vectors":vectors.len(), "inference":inference,
        "dimensions":vectors["en"].vector.len(),"cross_language_cosine":cosine(&vectors["en"].vector,&vectors["zh"].vector)?,
        "relevance_evaluation":false,"purpose":"installed-model integration probe; no judged retrieval claim"})
    );
    Ok(())
}
