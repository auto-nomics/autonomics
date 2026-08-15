//! Dedicated KMS TUI command.

use crate::cli::KmsArgs;

pub fn run_kms(args: KmsArgs) -> color_eyre::Result<()> {
    let agent_db = args
        .agent_db
        .unwrap_or_else(|| runtime::RuntimeConfig::default().agent_db);
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let mut app = crate::kms_tui::KmsTui::new(&agent_db)
            .await
            .map_err(color_eyre::eyre::Report::msg)?;
        app.run().await?;
        Ok::<(), color_eyre::Report>(())
    })
}
