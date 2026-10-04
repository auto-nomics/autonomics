//! CLI definitions for the `autonomics export-run ...` subcommand:
//! export a recorded DAG run as standard provenance evidence.

use std::path::PathBuf;

use clap::Args;

#[derive(Debug, Args)]
pub struct ExportRunArgs {
    /// Run id (exact or unique prefix) — see `dag_runs_log` for recorded ids.
    pub run_id: String,

    /// Evidence format: `crate` (RO-Crate 1.1 directory: manifest + verified
    /// result files) or `prov` (single W3C PROV-JSON document).
    #[arg(long, default_value = "crate")]
    pub format: String,

    /// Absolute output directory (holds the crate; or the `prov-<id>.json`
    /// document for `--format prov`).
    #[arg(long, value_name = "PATH")]
    pub out: PathBuf,

    /// State directory holding `dag-history.db` (defaults to ~/.autonomics).
    #[arg(long, value_name = "PATH")]
    pub state_dir: Option<PathBuf>,
}
