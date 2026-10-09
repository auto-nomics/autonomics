//! The local ONNX provider: a real embedding model behind the shared
//! contract.
//!
//! Loads a Hugging Face-style export — `tokenizer.json`, `config.json`,
//! and a `.onnx` graph (quantized weights may live in an external
//! `.onnx_data` sibling) — and serves [`TextEmbedder`] from it.
//! Encoding, pooling, and normalization happen here in fixed code:
//!
//! 1. tokenize with the model's own tokenizer, truncated at the
//!    configured max length
//! 2. run the graph — batched when the caller asks for a batch, which
//!    is why [`TextEmbedder::batch_embed`] is overridden
//! 3. pool: masked mean over the last hidden state, or the graph's
//!    own `[batch, hidden]` output when it exports one
//! 4. L2-normalize (idempotent when the graph already normalized)
//!
//! The model signature is derived from the model directory name, the
//! ONNX artifact stem, and the hidden width. Quantized and
//! full-precision exports of the same model get different signatures
//! on purpose: their vectors are not comparable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::Tensor;
use serde::Deserialize;
use tokenizers::{Encoding, Tokenizer, TruncationParams};

use crate::error::{EmbeddingError, Result};
use crate::input::EmbeddingInput;
use crate::output::EmbeddingOutput;
use crate::provider::TextEmbedder;
use crate::vector::normalize;

/// Default truncation length, in tokens.
pub const DEFAULT_MAX_LENGTH: usize = 512;

/// Options for opening a local ONNX embedding model.
#[derive(Debug, Clone)]
pub struct OnnxOptions {
    /// Maximum tokenized length; longer inputs are truncated.
    pub max_length: usize,
    /// Intra-op threads for the session; `None` uses the runtime
    /// default (one thread per core).
    pub intra_threads: Option<usize>,
}

impl Default for OnnxOptions {
    fn default() -> Self {
        Self {
            max_length: DEFAULT_MAX_LENGTH,
            intra_threads: None,
        }
    }
}

/// `hidden_size` is the only field we need from the model config.
#[derive(Debug, Deserialize)]
struct ModelConfig {
    hidden_size: Option<usize>,
}

/// A local ONNX embedding model, loaded once and reused.
pub struct OnnxTextEmbedder {
    signature: String,
    max_length: usize,
    pad_token_id: u32,
    tokenizer: Tokenizer,
    /// ONNX Runtime runs take `&mut self`; the mutex serializes
    /// concurrent embedders instead of the session being `Sync`.
    session: Mutex<Session>,
    /// Whether the graph also expects `token_type_ids` (BERT-style
    /// inputs); Gemma-style graphs do not.
    uses_token_type_ids: bool,
    /// Name of the graph output carrying the embeddings.
    output_name: String,
}

impl OnnxTextEmbedder {
    /// Open the model directory with default options.
    pub fn open(model_dir: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(model_dir, OnnxOptions::default())
    }

    /// Open the model directory with explicit options.
    pub fn open_with(model_dir: impl AsRef<Path>, options: OnnxOptions) -> Result<Self> {
        let model_dir = model_dir.as_ref();
        let onnx_path = resolve_onnx(model_dir)?;
        let tokenizer_path = model_dir.join("tokenizer.json");
        if !tokenizer_path.is_file() {
            return Err(EmbeddingError::Provider(format!(
                "no tokenizer.json under {}",
                model_dir.display()
            )));
        }

        let mut tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| {
            EmbeddingError::Provider(format!(
                "cannot load tokenizer from {}: {e}",
                tokenizer_path.display()
            ))
        })?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: options.max_length,
                ..TruncationParams::default()
            }))
            .map_err(|e| EmbeddingError::Provider(format!("cannot set truncation: {e}")))?;
        let pad_token_id = tokenizer.token_to_id("<pad>").ok_or_else(|| {
            EmbeddingError::Provider("tokenizer has no <pad> token".into())
        })?;
        let hidden_size = read_hidden_size(model_dir)?;

        let builder = Session::builder().map_err(onnx_error("cannot create session builder"))?;
        let builder = builder
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(onnx_error("cannot set optimization level"))?;
        let mut builder = match options.intra_threads {
            Some(threads) => builder
                .with_intra_threads(threads)
                .map_err(onnx_error("cannot set intra threads"))?,
            None => builder,
        };
        let session = builder
            .commit_from_file(&onnx_path)
            .map_err(onnx_error("cannot load model"))?;

        let uses_token_type_ids = session
            .inputs()
            .iter()
            .any(|input| input.name() == "token_type_ids");
        let output_name = session
            .outputs()
            .first()
            .map(|output| output.name().to_string())
            .ok_or_else(|| EmbeddingError::Provider("model has no outputs".into()))?;

        // Model identity: directory + artifact + width. The artifact
        // stem distinguishes the quantized export from the
        // full-precision one — their vectors are not interchangeable.
        let dir_name = model_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("model");
        let artifact = onnx_path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("model");
        let signature = match hidden_size {
            Some(hidden) => format!("{dir_name}-{artifact}-{hidden}"),
            None => format!("{dir_name}-{artifact}"),
        };

        Ok(Self {
            signature,
            max_length: options.max_length,
            pad_token_id,
            tokenizer,
            session: Mutex::new(session),
            uses_token_type_ids,
            output_name,
        })
    }

    /// The model signature stamped on every vector this embedder
    /// produces.
    pub fn signature(&self) -> &str {
        &self.signature
    }

    /// The truncation length this embedder tokenizes at.
    pub fn max_length(&self) -> usize {
        self.max_length
    }

    /// One forward pass over the batch — pad, run, pool, normalize.
    /// Output vectors come back in input order.
    fn run_batch(&self, encodings: &[Encoding]) -> Result<Vec<Vec<f32>>> {
        let batch = encodings.len();
        let seq_len = encodings
            .iter()
            .map(|encoding| encoding.get_ids().len())
            .max()
            .unwrap_or(0)
            .max(1);
        let rows: Vec<(&[u32], &[u32])> = encodings
            .iter()
            .map(|encoding| (encoding.get_ids(), encoding.get_attention_mask()))
            .collect();
        let (mut ids, mask) = pad_batch(&rows, self.pad_token_id, seq_len);

        let mut session = self
            .session
            .lock()
            .map_err(|_| EmbeddingError::Provider("onnx session lock poisoned".into()))?;
        let mask_input = mask.clone();
        let outputs = if self.uses_token_type_ids {
            let type_ids = vec![0_i64; batch * seq_len];
            session
                .run(ort::inputs![
                    "input_ids" => Tensor::from_array((vec![batch, seq_len], ids)).map_err(onnx_error("cannot build input_ids"))?,
                    "attention_mask" => Tensor::from_array((vec![batch, seq_len], mask_input)).map_err(onnx_error("cannot build attention_mask"))?,
                    "token_type_ids" => Tensor::from_array((vec![batch, seq_len], type_ids)).map_err(onnx_error("cannot build token_type_ids"))?,
                ])
                .map_err(onnx_error("inference failed"))?
        } else {
            ids.shrink_to_fit();
            session
                .run(ort::inputs![
                    "input_ids" => Tensor::from_array((vec![batch, seq_len], ids)).map_err(onnx_error("cannot build input_ids"))?,
                    "attention_mask" => Tensor::from_array((vec![batch, seq_len], mask_input)).map_err(onnx_error("cannot build attention_mask"))?,
                ])
                .map_err(onnx_error("inference failed"))?
        };

        let (shape, data) = outputs[self.output_name.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(onnx_error("cannot read output tensor"))?;
        let dims: Vec<usize> = shape.iter().map(|&d| d as usize).collect();
        let mut vectors = match dims.as_slice() {
            [b, seq, hidden] if *b == batch => pool_rows(data, &mask, *seq, *hidden),
            [b, hidden] if *b == batch => direct_rows(data, *hidden),
            other => {
                return Err(EmbeddingError::Provider(format!(
                    "unexpected output shape {other:?} for batch {batch}"
                )))
            }
        };
        for vector in &mut vectors {
            normalize(vector);
        }
        Ok(vectors)
    }
}

impl TextEmbedder for OnnxTextEmbedder {
    fn embed(&self, input: &EmbeddingInput) -> Result<EmbeddingOutput> {
        let encoding = self
            .tokenizer
            .encode(input.text.as_str(), true)
            .map_err(|e| EmbeddingError::Provider(format!("cannot tokenize input: {e}")))?;
        let vector = self.run_batch(std::slice::from_ref(&encoding))?.remove(0);
        Ok(EmbeddingOutput::new(&self.signature, vector))
    }

    /// A single batched forward pass for the whole input set — the
    /// reason a local model overrides the looping default.
    fn batch_embed(
        &self,
        inputs: &[EmbeddingInput],
    ) -> Result<BTreeMap<String, EmbeddingOutput>> {
        if inputs.is_empty() {
            return Ok(BTreeMap::new());
        }
        let texts: Vec<&str> = inputs.iter().map(|input| input.text.as_str()).collect();
        let encodings = self
            .tokenizer
            .encode_batch(texts, true)
            .map_err(|e| EmbeddingError::Provider(format!("cannot tokenize batch: {e}")))?;
        let vectors = self.run_batch(&encodings)?;
        Ok(inputs
            .iter()
            .map(|input| input.id.clone())
            .zip(vectors)
            .map(|(id, vector)| (id, EmbeddingOutput::new(&self.signature, vector)))
            .collect())
    }
}

/// Pick the ONNX graph deterministically: the canonical optimum names
/// first, then the alphabetically-first graph under `onnx/`, then a
/// flat graph in the directory itself.
fn resolve_onnx(model_dir: &Path) -> Result<PathBuf> {
    let mut candidates = Vec::new();
    let sub = model_dir.join("onnx");
    for name in ["model.onnx", "model_quantized.onnx"] {
        candidates.push(sub.join(name));
        candidates.push(model_dir.join(name));
    }
    for dir in [&sub, model_dir] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut names: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "onnx"))
            .collect();
        names.sort();
        candidates.extend(names);
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            EmbeddingError::Provider(format!(
                "no .onnx graph under {}",
                model_dir.display()
            ))
        })
}

fn read_hidden_size(model_dir: &Path) -> Result<Option<usize>> {
    let path = model_dir.join("config.json");
    let text = std::fs::read_to_string(&path).map_err(|e| {
        EmbeddingError::Provider(format!("cannot read {}: {e}", path.display()))
    })?;
    let config: ModelConfig = serde_json::from_str(&text).map_err(|e| {
        EmbeddingError::Provider(format!("cannot parse {}: {e}", path.display()))
    })?;
    Ok(config.hidden_size)
}

fn onnx_error<E: std::fmt::Display>(context: &'static str) -> impl Fn(E) -> EmbeddingError {
    move |e| EmbeddingError::Provider(format!("{context}: {e}"))
}

/// Right-pad every row to `seq_len`: ids with the pad token, mask with
/// 0 so pooling skips the filler.
fn pad_batch(
    rows: &[(&[u32], &[u32])],
    pad_token_id: u32,
    seq_len: usize,
) -> (Vec<i64>, Vec<i64>) {
    let mut ids = Vec::with_capacity(rows.len() * seq_len);
    let mut mask = Vec::with_capacity(rows.len() * seq_len);
    for (row_ids, row_mask) in rows {
        ids.extend(row_ids.iter().map(|&token| token as i64));
        mask.extend(row_mask.iter().map(|&flag| flag as i64));
        let padding = seq_len.saturating_sub(row_ids.len());
        ids.extend(std::iter::repeat_n(pad_token_id as i64, padding));
        mask.extend(std::iter::repeat_n(0, padding));
    }
    (ids, mask)
}

/// Masked mean-pool per row out of a packed `[batch, seq, hidden]`
/// slice. A fully-masked row pools to the zero vector.
fn pool_rows(data: &[f32], mask: &[i64], seq: usize, hidden: usize) -> Vec<Vec<f32>> {
    let batch = mask.len() / seq;
    (0..batch)
        .map(|b| {
            let mut sum = vec![0.0_f32; hidden];
            let mut count = 0.0_f32;
            for t in 0..seq {
                if mask[b * seq + t] == 0 {
                    continue;
                }
                count += 1.0;
                let row = &data[(b * seq + t) * hidden..(b * seq + t + 1) * hidden];
                for (total, value) in sum.iter_mut().zip(row) {
                    *total += value;
                }
            }
            if count > 0.0 {
                for value in &mut sum {
                    *value /= count;
                }
            }
            sum
        })
        .collect()
}

/// Slice a packed `[batch, hidden]` output into per-row vectors.
fn direct_rows(data: &[f32], hidden: usize) -> Vec<Vec<f32>> {
    data.chunks(hidden).map(<[f32]>::to_vec).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_batch_fills_ids_and_zeroes_the_mask() {
        let (ids, mask) = pad_batch(&[(&[5, 6], &[1, 1]), (&[7], &[1])], 0, 3);
        assert_eq!(ids, vec![5, 6, 0, 7, 0, 0]);
        assert_eq!(mask, vec![1, 1, 0, 1, 0, 0]);
    }

    #[test]
    fn pool_rows_means_only_masked_positions() {
        // batch 2, seq 3, hidden 2; row 1 masks out its middle token.
        let data = vec![
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, // row 0
            10.0, 20.0, 999.0, 999.0, 30.0, 40.0, // row 1
        ];
        let pooled = pool_rows(&data, &[1, 1, 1, 1, 0, 1], 3, 2);
        assert_eq!(pooled[0], vec![3.0, 4.0]);
        assert_eq!(pooled[1], vec![20.0, 30.0]);
    }

    #[test]
    fn a_fully_masked_row_pools_to_zero() {
        let data = vec![1.0, 2.0];
        let pooled = pool_rows(&data, &[0], 1, 2);
        assert_eq!(pooled[0], vec![0.0, 0.0]);
    }

    #[test]
    fn direct_rows_chunks_the_packed_output() {
        let rows = direct_rows(&[1.0, 2.0, 3.0, 4.0], 2);
        assert_eq!(rows, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
    }

    /// The trait requires `Send + Sync`: the session lives behind a
    /// mutex, so the embedder can move into the skills manager.
    #[test]
    fn onnx_embedder_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OnnxTextEmbedder>();
    }
}

#[cfg(test)]
mod real_model_tests {
    use super::*;

    /// Overridable so other machines can point at any Gemma-style
    /// export: `TEXT_EMBEDDING_TEST_MODEL_DIR=... cargo test -- --ignored`.
    fn model_dir() -> PathBuf {
        std::env::var_os("TEXT_EMBEDDING_TEST_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/mnt/base/models/harrier-oss-v1-270m"))
    }

    #[test]
    #[ignore = "requires the local ONNX embedding model"]
    fn real_model_embeds_and_separates_topics() {
        let embedder = OnnxTextEmbedder::open(model_dir()).unwrap();

        let a = embedder
            .embed(&EmbeddingInput::new(
                "a",
                "restart the daemon after editing config",
            ))
            .unwrap();
        let b = embedder
            .embed(&EmbeddingInput::new(
                "b",
                "restart the daemon once the config changed",
            ))
            .unwrap();
        let c = embedder
            .embed(&EmbeddingInput::new(
                "c",
                "quote mixed-case column names in sql",
            ))
            .unwrap();

        // Deterministic and stamped.
        let again = embedder
            .embed(&EmbeddingInput::new(
                "a",
                "restart the daemon after editing config",
            ))
            .unwrap();
        assert_eq!(a, again);
        assert_eq!(a.model_signature, embedder.signature());
        assert_eq!(a.model_signature, b.model_signature);

        // Paraphrases rank closer than an unrelated topic.
        let close = crate::cosine(&a.vector, &b.vector).unwrap();
        let far = crate::cosine(&a.vector, &c.vector).unwrap();
        assert!(
            close > far,
            "paraphrase similarity {close} must beat unrelated {far}"
        );
        assert!(close > 0.5, "paraphrases should be similar, got {close}");

        // The batched path agrees with the single path (padding is
        // masked out of pooling, so only float noise may differ).
        let batch = embedder
            .batch_embed(&[
                EmbeddingInput::new(
                    "a",
                    "restart the daemon after editing config",
                ),
                EmbeddingInput::new(
                    "b",
                    "restart the daemon once the config changed",
                ),
            ])
            .unwrap();
        let batch_vs_single = crate::cosine(&batch["a"].vector, &a.vector).unwrap();
        assert!(
            batch_vs_single > 0.999,
            "batched vector must match single, cosine {batch_vs_single}"
        );
    }
}
