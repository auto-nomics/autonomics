//! AlphaGenome Atlas DAG node bundle.
//!
//! Source nodes that pull precomputed variant-impact scores from the
//! [AlphaGenome Atlas](https://alphagenome.google/atlas) (GRCh38 SNVs: the
//! AVI score and per-track scores) and emit them as DataFusion DataFrames in
//! tidy long format, so they can flow through a DAG (filter via `sql_node`,
//! join with credible sets, sink to CSV/VFS, …).
//!
//! Requires the `ALPHAGENOME_API_KEY` environment variable.
//!
//! # Nodes
//!
//! | Kind | Input | Output |
//! |------|-------|--------|
//! | `source_alphagenome_scores` | variant list in spec | variant × scorer × track rows |
//! | `alphagenome_scores` | DataFrame variant column | variant × scorer × track rows |
//! | `source_alphagenome_interval_avi` | genomic window in spec | top SNVs by AVI with Phred |

pub mod interval_avi;
pub mod scores;
pub mod shared;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "alphagenome"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(scores::AlphagenomeScoresSourceNodeFactory));
        registry.register(Box::new(scores::AlphagenomeScoresNodeFactory));
        registry.register(Box::new(interval_avi::AlphagenomeIntervalAviNodeFactory));
    }
}

#[cfg(test)]
mod tests {
    use alphagenome::types::{VariantEcho, VariantScoreArray, VariantScores};

    use super::shared::{build_filter, build_score_batch, error_row, flatten_scores, ScoreRow};

    fn fake_scores() -> VariantScores {
        VariantScores {
            variant: VariantEcho {
                chromosome: "chr22".to_owned(),
                position: 36_201_698,
                reference_bases: "A".to_owned(),
                alternate_bases: "C".to_owned(),
            },
            interval: None,
            scores: vec![
                VariantScoreArray {
                    scorer: "AVI_SCORE".to_owned(),
                    is_signed: false,
                    shape: vec![1],
                    values: vec![1.25],
                    calibrated_values: Some(vec![0.995]),
                    tracks: vec![],
                    genes: vec![],
                },
                VariantScoreArray {
                    scorer: "RNA_SEQ".to_owned(),
                    is_signed: true,
                    shape: vec![2],
                    values: vec![0.4, -0.3],
                    calibrated_values: None,
                    tracks: vec![
                        alphagenome::TrackInfo {
                            name: "colon:RNA_SEQ".to_owned(),
                            biosample: Some("colon".to_owned()),
                            ontology_curie: Some("UBERON:0001157".to_owned()),
                            ..Default::default()
                        },
                        alphagenome::TrackInfo {
                            name: "liver:RNA_SEQ".to_owned(),
                            biosample: Some("liver".to_owned()),
                            ontology_curie: None,
                            ..Default::default()
                        },
                    ],
                    genes: vec![],
                },
            ],
        }
    }

    #[test]
    fn flatten_produces_one_row_per_value() {
        let rows = flatten_scores(&fake_scores());
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].scorer, "AVI_SCORE");
        assert!((rows[0].raw_score.unwrap() - 1.25).abs() < f32::EPSILON as f64);
        assert_eq!(rows[1].track.as_deref(), Some("colon:RNA_SEQ"));
        assert_eq!(rows[1].ontology_curie.as_deref(), Some("UBERON:0001157"));
        assert_eq!(rows[2].biosample.as_deref(), Some("liver"));
    }

    #[test]
    fn error_rows_survive_batch_build() {
        let rows = vec![error_row("chr1", 100, "A", "T", "not found".into())];
        let batch = build_score_batch(&rows).unwrap();
        assert_eq!(batch.num_rows(), 1);
        assert!(batch.column_by_name("raw_score").unwrap().is_null(0));
        assert!(!batch.column_by_name("error").unwrap().is_null(0));
    }

    #[test]
    fn filter_builders() {
        assert_eq!(
            build_filter(&["AVI_SCORE".to_owned()], &[]),
            "(scores.variant_scorer.name = \"AVI_SCORE\")"
        );
        let with_onto = build_filter(
            &["RNA_SEQ".to_owned()],
            &["UBERON:0001157".to_owned(), "junk".to_owned()],
        );
        assert!(with_onto.contains("ONTOLOGY_TYPE_UBERON AND"));
        assert!(with_onto.contains("ontology_term.id = 1157"));
        assert!(with_onto.starts_with("(scores.variant_scorer.name"));
        assert!(with_onto.contains("-scores.metadata.tracks.metadata.ontology_term:* OR"));
    }

    #[test]
    fn scores_specs_deserialise() {
        let spec: crate::scores::AlphagenomeScoresSourceSpec = serde_json::from_value(
            serde_json::json!({"variants": ["chr22:36201698:A>C"]}),
        )
        .unwrap();
        assert_eq!(spec.scorers.scorers, vec!["AVI_SCORE"]);
        let spec: crate::scores::AlphagenomeScoresSpec =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(spec.variant_column, "variant");
        let spec: crate::interval_avi::AlphagenomeIntervalAviSpec = serde_json::from_value(
            serde_json::json!({"interval": "chr11:5225727-5226575"}),
        )
        .unwrap();
        assert_eq!(spec.top_k, 25);
        assert_eq!(spec.page_size, 256);
    }

    #[test]
    fn build_score_batch_empty_errors() {
        let rows: Vec<ScoreRow> = vec![];
        assert!(build_score_batch(&rows).is_err());
    }
}
