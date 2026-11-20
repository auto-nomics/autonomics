//! Async Rust SDK for the AlphaGenome API (DeepMind).
//!
//! Two services are exposed over gRPC at `gdmscience.googleapis.com:443`
//! (auth via the `x-goog-api-key` metadata header):
//!
//! * **Atlas** (`AtlasService`) — precomputed variant scores for every GRCh38
//!   SNV: the AVI score, its feature attributions, and per-track scores.
//! * **DNA model** (`DnaModelService`) — on-demand variant / interval scoring
//!   and prediction with the AlphaGenome model (human and mouse).
//!
//! ```no_run
//! use alphagenome::{AlphaGenomeClient, Organism, VariantQuery};
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), alphagenome::Error> {
//! let mut client = AlphaGenomeClient::from_env().await?;
//! let scores = client
//!     .atlas_variant_scores(
//!         &"chr22:36201698:A>C".parse::<VariantQuery>()?,
//!         Some("scores.variant_scorer.name = \"AVI_SCORE\""),
//!     )
//!     .await?;
//! for score in &scores.scores {
//!     println!("{}: {:?}", score.scorer, &score.values[..score.values.len().min(4)]);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Coordinate contract (mirrors the official Python SDK):
//! * variants are 1-based `chr:pos:ref>alt`; gnomAD (`chr-pos-ref-alt`) and
//!   GTEx (`chr_pos_ref_alt_b38`) spellings are autodetected by
//!   [`VariantQuery::from_str`];
//! * `IntervalQuery` is 1-based closed on input and converted to the
//!   0-based half-open wire format;
//! * human is GRCh38 only; Atlas scores are SNV-only.

pub mod client;
pub mod error;
pub mod pb;
pub mod query;
pub mod tensor;
pub mod types;

pub use client::{AlphaGenomeClient, DEFAULT_ENDPOINT, ENV_API_KEY};
pub use error::{Error, Result};
pub use pb::v1::{Organism, OutputType};
pub use query::{IntervalQuery, VariantQuery};
pub use types::{ScoredOutput, ScorerMetadataEntry, TrackInfo, VariantScoreArray, VariantScores};
