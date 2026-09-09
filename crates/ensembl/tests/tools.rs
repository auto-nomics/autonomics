use std::sync::Arc;

use ensembl::{EnsemblClient, ensembl_registrations};

#[test]
fn registrations_have_unique_nonempty_schemas() {
    let registrations = ensembl_registrations(Arc::new(EnsemblClient::new()));
    assert_eq!(registrations.len(), 8);
    let mut names: Vec<_> = registrations
        .iter()
        .map(|registration| registration.definition.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), registrations.len());
    assert!(
        registrations
            .iter()
            .all(|registration| { !registration.definition.input_schema.properties.is_empty() })
    );
}
