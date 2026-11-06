//! OpenGWAS source DAG node bundle.
//!
//! Eight source nodes that pull tabular data from the [OpenGWAS](
//! https://gwas-api.mrcieu.ac.uk/) REST API and emit it as DataFusion
//! `DataFrame`s, so GWAS association tables, variant annotations, study
//! metadata, etc. can flow through a DAG (join, filter via `sql_node`,
//! sink to CSV/VFS, …).
//!
//! All are single-output source nodes that reuse the `opengwas` SDK client
//! and require the `OPENGWAS_TOKEN` environment variable. Most are
//! zero-input (IDs embedded in the spec); the exception is `opengwas_phewas`,
//! which reads its variant list from an upstream DataFrame column.
//!
//! # Nodes
//!
//! | Kind | Endpoint | Output |
//! |------|----------|--------|
//! | `source_opengwas_associations` | `/associations` | variant × study associations |
//! | `opengwas_phewas` | `/phewas` (DataFrame input) | variant × trait PheWAS hits |
//! | `source_opengwas_gwasinfo` | `/gwasinfo` | study metadata by ID |
//! | `source_opengwas_gwasinfo_search` | `/gwasinfo` (SQL search) | matching study metadata |
//! | `source_opengwas_variants_rsid` | `/variants/rsid` | variant annotations by rsID |
//! | `source_opengwas_variants_chrpos` | `/variants/chrpos` | variant annotations by chr:pos |
//! | `source_opengwas_ld_clump` | `/ld/clump` | clumped independent loci |
//! | `source_opengwas_tophits` | `/tophits` | top-associated SNPs (optionally clumped) |

pub mod associations;
pub mod count_matrix;
pub mod gwasinfo;
pub mod gwasinfo_search;
pub mod ld_clump;
pub mod phewas;
pub mod shared;
pub mod tophits;
pub mod variants_chrpos;
pub mod variants_rsid;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "opengwas"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(associations::OpengwasAssociationsNodeFactory));
        registry.register(Box::new(phewas::OpengwasPhewasNodeFactory));
        registry.register(Box::new(gwasinfo::OpengwasGwasinfoNodeFactory));
        registry.register(Box::new(gwasinfo_search::OpengwasGwasinfoSearchNodeFactory));
        registry.register(Box::new(variants_rsid::OpengwasVariantsRsidNodeFactory));
        registry.register(Box::new(variants_chrpos::OpengwasVariantsChrposNodeFactory));
        // M3-② spike: the scalar and matrix operations the OpenGWAS tool
        // layer kept back, folded into the DataFrame channel (T2 rule —
        // scalar = single-row DataFrame, matrix = long-format DataFrame).
        registry.register(Box::new(count_matrix::OpengwasGwasinfoCountNodeFactory));
        registry.register(Box::new(count_matrix::OpengwasLdMatrixNodeFactory));
        registry.register(Box::new(ld_clump::OpengwasLdClumpNodeFactory));
        registry.register(Box::new(tophits::OpengwasTophitsNodeFactory {}));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::{Array, Float64Array, Int64Array, StringArray};
    use arrow_schema::DataType;
    use opengwas::types::GwasInfo;
    use serde_json::Value;

    use crate::shared::{
        build_gwasinfo_batch, build_json_batch, extract_rows, infer_columns, string_column_values,
    };
    use crate::{associations, gwasinfo_search, ld_clump, phewas};

    // -- extract_rows --

    #[test]
    fn extract_rows_flat_array() {
        let val = serde_json::json!([
            {"rsid": "rs1", "pval": 1e-8},
            {"rsid": "rs2", "pval": 1e-9},
        ]);
        let rows = extract_rows(&val);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn extract_rows_keyed_object() {
        let val = serde_json::json!({
            "ieu-a-2": [{"rsid": "rs1"}],
            "ukb-b-1": [{"rsid": "rs2"}, {"rsid": "rs3"}],
        });
        let rows = extract_rows(&val);
        assert_eq!(rows.len(), 3);
    }

    // -- infer_columns --

    #[test]
    fn infer_columns_priority_order() {
        let rows = extract_rows(&serde_json::json!([
            {"samplesize": 1000, "rsid": "rs1", "pval": 5e-8, "beta": 0.1, "zzz": "extra"},
        ]));
        let cols = infer_columns(&rows);
        assert_eq!(cols[0], "rsid");
        assert_eq!(cols.last().unwrap(), "zzz");
    }

    // -- build_json_batch --

    #[test]
    fn build_batch_infers_types() {
        let rows = extract_rows(&serde_json::json!([
            {"rsid": "rs1", "chr": 1, "position": 12345, "beta": 0.05, "pval": 5e-8},
            {"rsid": "rs2", "chr": 2, "position": 67890, "beta": -0.03, "pval": 1e-9},
        ]));
        let batch = build_json_batch(&rows).unwrap();
        assert_eq!(batch.num_rows(), 2);

        let chr = batch
            .column_by_name("chr")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(chr.value(0), 1);
        assert_eq!(chr.value(1), 2);

        let beta = batch
            .column_by_name("beta")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((beta.value(0) - 0.05).abs() < 1e-10);

        let rsid = batch
            .column_by_name("rsid")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(rsid.value(0), "rs1");
    }

    #[test]
    fn build_batch_handles_nulls() {
        let rows = extract_rows(&serde_json::json!([
            {"rsid": "rs1", "pval": 5e-8},
            {"rsid": "rs2", "pval": null},
        ]));
        let batch = build_json_batch(&rows).unwrap();
        let pval = batch
            .column_by_name("pval")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((pval.value(0) - 5e-8).abs() < 1e-20);
        assert!(pval.is_null(1));
    }

    #[test]
    fn build_batch_empty_rows_errors() {
        let rows: Vec<Value> = vec![];
        assert!(build_json_batch(&rows).is_err());
    }

    // -- build_gwasinfo_batch --

    #[test]
    fn gwasinfo_batch_basic() {
        let rows = vec![
            GwasInfo {
                id: Some("ieu-a-2".into()),
                trait_: Some("CAD".into()),
                year: Some(2015),
                nsnp: Some(9455_552),
                sample_size: Some(184_305),
                sd: Some(1.0),
                ..Default::default()
            },
            GwasInfo {
                id: Some("ukb-b-19953".into()),
                trait_: Some("BMI".into()),
                year: Some(2020),
                ..Default::default()
            },
        ];
        let batch = build_gwasinfo_batch(&rows).unwrap();
        assert_eq!(batch.num_rows(), 2);

        let id = batch
            .column_by_name("id")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(id.value(0), "ieu-a-2");
        assert_eq!(id.value(1), "ukb-b-19953");

        let year = batch
            .column_by_name("year")
            .unwrap()
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(year.value(0), 2015);
        assert_eq!(year.value(1), 2020);

        let sd = batch
            .column_by_name("sd")
            .unwrap()
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!((sd.value(0) - 1.0).abs() < f64::EPSILON);
        assert!(sd.is_null(1));
    }

    #[test]
    fn gwasinfo_batch_empty_errors() {
        let rows: Vec<GwasInfo> = vec![];
        assert!(build_gwasinfo_batch(&rows).is_err());
    }

    // -- spec deserialization --

    #[test]
    fn associations_spec_deserialises() {
        let json = serde_json::json!({
            "variant": ["rs1205"],
            "id": ["ieu-a-2"]
        });
        let spec: associations::OpengwasAssociationsSpec = serde_json::from_value(json).unwrap();
        assert_eq!(spec.variant, vec!["rs1205"]);
        assert_eq!(spec.id, vec!["ieu-a-2"]);
    }

    #[test]
    fn phewas_spec_defaults() {
        let spec: phewas::OpengwasPhewasSpec =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(spec.variant_column, "rsid");
        assert!((spec.pval - 0.01).abs() < f64::EPSILON);
        assert_eq!(spec.batch_size, 100);
        assert!(spec.index_list.is_none());
    }

    #[test]
    fn phewas_spec_explicit() {
        let json = serde_json::json!({
            "variant_column": "snp",
            "pval": 1e-4,
            "index_list": ["ieu-a-2"],
            "batch_size": 20
        });
        let spec: phewas::OpengwasPhewasSpec = serde_json::from_value(json).unwrap();
        assert_eq!(spec.variant_column, "snp");
        assert!((spec.pval - 1e-4).abs() < f64::EPSILON);
        assert_eq!(spec.index_list, Some(vec!["ieu-a-2".to_string()]));
        assert_eq!(spec.batch_size, 20);
    }

    // -- string_column_values (phewas DataFrame input) --

    fn batch_with_rsid() -> arrow_array::RecordBatch {
        use arrow_schema::{Field, Schema};
        let schema = Arc::new(Schema::new(vec![Field::new("rsid", DataType::Utf8, true)]));
        arrow_array::RecordBatch::try_new(
            schema,
            vec![Arc::new(StringArray::from(vec![
                Some("rs1"),
                None,
                Some("  "),
                Some("rs2"),
                Some("rs1"),
            ]))],
        )
        .unwrap()
    }

    #[test]
    fn string_column_dedups_and_drops_blanks() {
        let batches = vec![batch_with_rsid()];
        let vals = string_column_values("test", &batches, "rsid").unwrap();
        assert_eq!(vals, vec!["rs1", "rs2"]);
    }

    #[test]
    fn string_column_missing_column_errors() {
        let batches = vec![batch_with_rsid()];
        let err = string_column_values("test", &batches, "nope").unwrap_err();
        assert!(err.to_string().contains("no column 'nope'"));
    }

    #[test]
    fn string_column_non_string_errors() {
        use arrow_schema::{Field, Schema};
        let schema = Arc::new(Schema::new(vec![Field::new("n", DataType::Int64, true)]));
        let batch = arrow_array::RecordBatch::try_new(
            schema,
            vec![Arc::new(Int64Array::from(vec![Some(1), None]))],
        )
        .unwrap();
        let err = string_column_values("test", &[batch], "n").unwrap_err();
        assert!(err.to_string().contains("must be a string column"));
    }

    #[test]
    fn gwasinfo_search_spec_defaults() {
        let json = serde_json::json!({"keyword": "diabetes"});
        let spec: gwasinfo_search::OpengwasGwasinfoSearchSpec =
            serde_json::from_value(json).unwrap();
        assert_eq!(spec.field, "trait");
        assert_eq!(spec.limit, 50);
    }

    #[test]
    fn ld_clump_spec_defaults() {
        let json = serde_json::json!({"rsid": ["rs1"], "pval": [1e-8]});
        let spec: ld_clump::OpengwasLdClumpSpec = serde_json::from_value(json).unwrap();
        assert!((spec.r2 - 0.001).abs() < f64::EPSILON);
        assert_eq!(spec.kb, 5000);
        assert_eq!(spec.pop, "EUR");
    }
}
