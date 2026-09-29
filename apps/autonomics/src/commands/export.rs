//! Export a recorded DAG run as deliverable provenance evidence.
//!
//! Offline path: opens `dag-history.db` directly (multiprocess WAL allows
//! coexistence with a running daemon) and pulls packaged files through the
//! same `state_dir/vfs.toml` mount rules as the engine.

use color_eyre::eyre::{Context, bail};

use crate::cli::ExportRunArgs;

pub async fn run_export_run(args: ExportRunArgs) -> color_eyre::Result<()> {
    use dag_core::dag::{DagHistory, ExportFormat, export as dag_export};

    let format =
        ExportFormat::parse(&args.format).map_err(|e| color_eyre::eyre::eyre!(e))?;
    if !args.out.is_absolute() {
        bail!("--out must be an absolute path, got `{}`", args.out.display());
    }

    let mut builder = gateway::RuntimeConfig::builder();
    if let Some(state_dir) = &args.state_dir {
        builder = builder.state_dir(state_dir);
    }
    let config = builder.build();

    let history = DagHistory::open(&config.dag_history_db)
        .await
        .context("open DAG history database")?;
    let run = dag_export::resolve_run(&history, &args.run_id)
        .await
        .map_err(|e| color_eyre::eyre::eyre!(e))?;
    // Engine-level error rows carry no run report; export what exists.
    let report: serde_json::Value = run
        .run_report_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let manifest = match &run.snapshot_id {
        Some(snapshot_id) => history
            .get_snapshot(snapshot_id)
            .await
            .context("fetch snapshot")?
            .map(|snapshot| snapshot.manifest_json),
        None => None,
    };

    let summary = match format {
        ExportFormat::Prov => dag_export::write_prov_document(
            &run,
            &report,
            manifest.as_deref(),
            &args.out,
        )
        .map_err(|e| color_eyre::eyre::eyre!(e))?,
        ExportFormat::Crate => {
            let storage = gateway::bibliography_file_storage(&config)?;
            dag_export::export_ro_crate(
                &run,
                &report,
                manifest.as_deref(),
                &args.out,
                Some(storage.as_ref()),
            )
            .await
            .map_err(|e| color_eyre::eyre::eyre!(e))?
        }
    };

    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
