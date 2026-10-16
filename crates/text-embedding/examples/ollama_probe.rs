//! Embed a few phrases with a local Ollama model and print dimensions.

use text_embedding::{EmbeddingInput, OllamaEmbedder, TextEmbedder, cosine};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let base_url = args
        .next()
        .unwrap_or_else(|| "http://127.0.0.1:11434".into());
    let model = args.next().unwrap_or_else(|| "qwen3-embedding:0.6b".into());
    let embedder = OllamaEmbedder::new(&base_url, &model);
    let inputs = [
        EmbeddingInput::new("a", "restart the daemon after editing config"),
        EmbeddingInput::new("b", "quote mixed-case column names in sql"),
    ];
    let outputs = embedder
        .batch_embed(&inputs)
        .await
        .expect("embedding failed");
    for input in inputs {
        let output = &outputs[&input.id];
        println!("{}: dim={}", output.model_signature, output.vector.len());
    }
    println!(
        "cosine={:?}",
        cosine(&outputs["a"].vector, &outputs["b"].vector)
    );
}
