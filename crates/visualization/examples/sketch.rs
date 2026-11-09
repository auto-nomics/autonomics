//! Emit an editable example scene: cargo run -p visualization --example sketch.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let plan = serde_json::from_value(serde_json::json!({
        "message":"Ligand binding initiates a signaling response", "audience":"biology researchers",
        "nodes":[{"id":"ligand","label":"Ligand","column":0,"row":0,"shape":"ellipse"},{"id":"receptor","label":"Receptor","column":1,"row":0},{"id":"response","label":"Cellular response","column":2,"row":0}],
        "edges":[{"from":"ligand","to":"receptor","label":"binds"},{"from":"receptor","to":"response","label":"activates"}],
        "assumptions":["Generic conceptual sketch; biological mechanism requires review"]
    }))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&visualization::sketch::compile(&plan)?)?
    );
    Ok(())
}
