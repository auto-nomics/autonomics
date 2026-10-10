//! Dedicated KMS TUI command.

use crate::cli::KmsArgs;

pub fn run_kms(args: KmsArgs) -> color_eyre::Result<()> {
    let defaults = gateway::RuntimeConfig::default();
    let kms_db = args.db.unwrap_or_else(|| defaults.kms_db_path.clone());
    let agent_db = args.agent_db.unwrap_or_else(|| defaults.agent_db.clone());
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let mut app = crate::kms_tui::KmsTui::new(&kms_db, &agent_db)
            .await
            .map_err(color_eyre::eyre::Report::msg)?;
        app.run().await?;
        Ok::<(), color_eyre::eyre::Report>(())
    })
}
