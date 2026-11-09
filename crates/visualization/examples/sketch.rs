//! Emit an editable example scene: cargo run -p visualization --example sketch.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut source = serde_json::json!({
        "message":"Ligand binding initiates a signaling response", "audience":"biology researchers",
        "nodes":[{"id":"ligand","label":"Ligand","column":0,"row":0,"shape":"ellipse"},{"id":"receptor","label":"Receptor","column":1,"row":0},{"id":"response","label":"Cellular response","column":2,"row":0}],
        "edges":[{"from":"ligand","to":"receptor","label":"binds"},{"from":"receptor","to":"response","label":"activates"}],
        "assumptions":["Generic conceptual sketch; biological mechanism requires review"]
    });
    if let Some(layout) = std::env::args().nth(1) {
        source["layout"] = serde_json::json!(layout);
        if layout != "grid" {
            for node in source["nodes"].as_array_mut().unwrap() {
                node.as_object_mut().unwrap().remove("column");
                node.as_object_mut().unwrap().remove("row");
            }
        }
    }
    let plan = serde_json::from_value(source)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&visualization::sketch::compile(&plan)?)?
    );
    Ok(())
}
