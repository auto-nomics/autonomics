//! Opt-in live source retrieval through the same registered agent tool. No figure writes.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input: serde_json::Value = serde_json::from_str(
        &std::env::args()
            .nth(1)
            .ok_or("provide a JSON figure_evidence request")?,
    )?;
    let tool = visualization_tools::evidence_registrations()
        .into_iter()
        .find(|tool| tool.definition.name == "figure_evidence")
        .ok_or("figure_evidence registration missing")?;
    let result = tool.implementation.execute(input).await?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
