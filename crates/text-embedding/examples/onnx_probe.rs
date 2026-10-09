//! Embed a few phrases with a local ONNX model and print the model
//! signature, width, and pairwise cosine similarities.
//!
//! Usage:
//!
//! ```sh
//! cargo run -p text-embedding --features onnx --example onnx_probe -- \
//!     /mnt/base/models/harrier-oss-v1-270m
//! ```

use std::time::Instant;

use text_embedding::{cosine, EmbeddingInput, OnnxTextEmbedder, TextEmbedder};

fn main() {
    let model_dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/mnt/base/models/harrier-oss-v1-270m".to_string());

    let started = Instant::now();
    let embedder = OnnxTextEmbedder::open(&model_dir).expect("cannot open model");
    println!(
        "loaded {} in {:.2}s (max_length {})",
        embedder.signature(),
        started.elapsed().as_secs_f32(),
        embedder.max_length()
    );

    let inputs = [
        ("daemon-restart", "restart the daemon after editing config"),
        ("daemon-restart-2", "restart the daemon once the config changed"),
        ("sql-quoting", "quote mixed-case column names in sql queries"),
    ];
    let inputs: Vec<EmbeddingInput> = inputs
        .iter()
        .map(|(id, text)| EmbeddingInput::new(*id, *text))
        .collect();

    let started = Instant::now();
    let outputs = embedder.batch_embed(&inputs).expect("embedding failed");
    println!(
        "embedded {} inputs in {:.3}s",
        inputs.len(),
        started.elapsed().as_secs_f32()
    );

    let get = |id: &str| outputs[id].vector.as_slice();
    println!("dim = {}", outputs["daemon-restart"].vector.len());
    for (a, b) in [("daemon-restart", "daemon-restart-2"), ("daemon-restart", "sql-quoting")] {
        println!("cosine({a}, {b}) = {:.4}", cosine(get(a), get(b)).unwrap());
    }
}
