//! Local VFS panel integration test.
//!
//! Ignored by default because it reads machine-local reference datasets.

use std::sync::Arc;
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::prelude::SessionContext;
use vfs::{BackendConfig, BackendDefinition, MountedObjectStore, MountDefinition, VfsManifest};

const LOCAL_PANEL_ROOT: &str =
    "/mnt/projects/autonomics_projects/autonomics/reference/ldsc_data/parquet";

fn panel_vfs() -> MountedObjectStore {
    let manifest = VfsManifest {
        backend: vec![BackendDefinition {
            id: "ldsc-local".into(),
            config: BackendConfig::local("/"),
        }],
        mount: vec![
            MountDefinition {
                path: "/data/ldsc".into(),
                backend: "ldsc-local".into(),
                source: LOCAL_PANEL_ROOT.into(),
                read_only: true,
            },
            MountDefinition {
                path: "/data/ukbb".into(),
                backend: "ldsc-local".into(),
                source: "/mnt/disk3/ld_score".into(),
                read_only: true,
            },
        ],
    };
    MountedObjectStore::from_manifest(&manifest).expect("valid VFS manifest")
}

#[tokio::test]
#[ignore = "reads local reference panels"]
async fn single_file_parquet_panels_resolve_through_vfs() {
    let ctx = SessionContext::new();
    ctx.runtime_env().register_object_store(
        ObjectStoreUrl::parse("vfs://").unwrap().as_ref(),
        Arc::new(panel_vfs()),
    );

    for (table, url) in [
        ("p1000", nodes_ldsc::ldsc_common::VFS_LDSCORE_1000G_EUR),
        ("p1000_m", nodes_ldsc::ldsc_common::VFS_LDSCORE_1000G_EUR_M),
        (
            "baseline",
            nodes_ldsc::ldsc_common::VFS_LDSCORE_BASELINELD_V2_2_EUR,
        ),
        (
            "baseline_m",
            nodes_ldsc::ldsc_common::VFS_LDSCORE_BASELINELD_V2_2_EUR_M,
        ),
    ] {
        nodes_ldsc::ldsc_common::register_listing_table(&ctx, table, url)
            .await
            .expect("register panel");
        let schema = ctx.table(table).await.unwrap().schema().clone();
        let names: Vec<_> = schema.fields().iter().map(|f| f.name().to_string()).collect();
        println!("{table}: {names:?}");
    }

    let df = ctx
        .sql(r#"SELECT "rsid", "ld_score", "w_ld" FROM p1000 ORDER BY locus.position LIMIT 1"#)
        .await
        .unwrap();
    let batches = df.collect().await.unwrap();
    assert_eq!(batches[0].num_rows(), 1);
}

#[tokio::test]
#[ignore = "reads local UKBB LD-score panels"]
async fn ukbb_panel_resolves_through_vfs() {
    let ctx = SessionContext::new();
    ctx.runtime_env().register_object_store(
        ObjectStoreUrl::parse("vfs://").unwrap().as_ref(),
        Arc::new(panel_vfs()),
    );

    nodes_ldsc::ldsc_common::register_listing_table(
        &ctx,
        "ukbb",
        nodes_ldsc::ldsc_common::VFS_LDSCORE_UKBB_EUR,
    )
    .await
    .unwrap();

    let df = ctx.sql(r#"SELECT "rsid", "ld_score" FROM ukbb LIMIT 1"#).await.unwrap();
    let batches = df.collect().await.unwrap();
    assert_eq!(batches[0].num_rows(), 1);
}
