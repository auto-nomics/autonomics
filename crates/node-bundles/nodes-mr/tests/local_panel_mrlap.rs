use std::sync::Arc;

use dag_core::DagNode;
use dag_core::dag::node_event::NodeReporter;
use dag_core::node::{DataBundle, NodeInput};
use dag_core::registry::NodeCtx;
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::prelude::{DataFrame, SessionContext};
use nodes_ldsc::ldsc_common::{self, BUNDLE_LDSCORE_1000G_EUR};
use nodes_mr::mrlap::{MrlapNode, MrlapSpec};
use vfs::{BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, VfsManifest};

const PANEL_DIRECTORY: &str = "/mnt/projects/autonomics_projects/autonomics/reference/ldsc_data";
const PANEL_VPATH: &str = "/reference/ldsc_data/parquet/1000g_eur.parquet";

fn session_and_ctx() -> (SessionContext, NodeCtx) {
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "local-mrlap-panel".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![MountDefinition {
            path: "/reference/ldsc_data".into(),
            backend: "local-mrlap-panel".into(),
            source: PANEL_DIRECTORY.into(),
            read_only: true,
        }],
    };
    let mounted = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
    let session = SessionContext::new();
    session
        .runtime_env()
        .register_object_store(ObjectStoreUrl::parse("vfs://").unwrap().as_ref(), mounted);
    let node_ctx = NodeCtx::new(session.runtime_env(), None);
    (session, node_ctx)
}

async fn probe_sumstats(session: &SessionContext, instrument_z: f64) -> DataFrame {
    session
        .sql(&format!(
            r#"
            SELECT
              rsid,
              CAST(locus.contig AS VARCHAR) AS chr,
              CAST(locus.position AS BIGINT) AS pos,
              CAST('A' AS VARCHAR) AS ea,
              CAST('G' AS VARCHAR) AS nea,
              CAST(
                CASE WHEN ROW_NUMBER() OVER () <= 20 THEN {instrument_z} ELSE 0.1 END
                AS DOUBLE
              ) AS z,
              CAST(400000.0 AS DOUBLE) AS n
            FROM source_panel
            LIMIT 50000
            "#
        ))
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "reads a machine-local 1000G LD-score panel"]
async fn string_view_contract_reaches_panel_join_beyond_harmonisation() {
    let (session, ctx) = session_and_ctx();
    let bundle = DataBundle::new(BUNDLE_LDSCORE_1000G_EUR, "1000G EUR", PANEL_VPATH);
    ldsc_common::register_listing_table(
        &session,
        "source_panel",
        &ldsc_common::storage_url(&bundle),
    )
    .await
    .unwrap();

    let exposure = probe_sumstats(&session, 8.0).await;
    let outcome = probe_sumstats(&session, 0.5).await;

    let mut node = MrlapNode::new(
        serde_json::from_value::<MrlapSpec>(serde_json::json!({
            "exposure_name": "probe_exposure",
            "outcome_name": "probe_outcome"
        }))
        .unwrap(),
        bundle,
    );
    let result = node
        .execute(
            &ctx,
            &[
                NodeInput::new_dataframe(0, exposure),
                NodeInput::new_dataframe(1, outcome),
            ],
            &NodeReporter::noop(),
        )
        .await
        .unwrap();

    let output = result.dataframe(0).unwrap();
    assert_eq!(output.clone().count().await.unwrap(), 1);
}
