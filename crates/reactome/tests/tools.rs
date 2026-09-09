use std::sync::Arc;

use reactome::{ReactomeClient, reactome_registrations};

#[test]
fn registrations_have_unique_nonempty_schemas() {
    let registrations = reactome_registrations(Arc::new(ReactomeClient::new()));
    assert_eq!(registrations.len(), 8);
    let mut names: Vec<_> = registrations
        .iter()
        .map(|registration| registration.definition.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), registrations.len());
    // `reactome_database` is intentionally parameterless; all others must
    // expose at least one input field.
    assert!(
        registrations
            .iter()
            .all(|registration| {
                registration.definition.name == "reactome_database"
                    || !registration.definition.input_schema.properties.is_empty()
            })
    );
}

#[test]
fn expected_tool_names_are_registered() {
    let registrations = reactome_registrations(Arc::new(ReactomeClient::new()));
    let names: Vec<&str> = registrations
        .iter()
        .map(|registration| registration.definition.name.as_str())
        .collect();
    for expected in [
        "reactome_database",
        "reactome_species",
        "reactome_top_pathways",
        "reactome_pathway_detail",
        "reactome_mapping",
        "reactome_participants",
        "reactome_search",
        "reactome_analysis",
    ] {
        assert!(names.contains(&expected), "missing tool: {expected}");
    }
}
