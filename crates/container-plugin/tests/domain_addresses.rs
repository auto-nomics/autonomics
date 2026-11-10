use container_plugin::node_definition::{NodeDefinition, validate};

#[test]
fn accepts_domain_artifact_prefixes() {
    let mut node: NodeDefinition = toml::from_str(
        r#"
kind = "domain_probe"
desc = "Domain publication probe"
doc = "Domain publication probe"
[ports]
outputs = [{ path = "result.txt" }]
[command]
interpreter = "true"
"#,
    )
    .unwrap();
    for prefix in [
        "ht://artifacts/probe",
        "local://artifacts/probe",
        "vfs:///artifacts/probe",
        "/artifacts/probe",
    ] {
        node.artifact_prefix = Some(prefix.into());
        validate(&node).unwrap();
    }
    node.artifact_prefix = Some("relative/path".into());
    assert!(validate(&node).is_err());
}
