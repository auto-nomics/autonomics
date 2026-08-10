//! Unit tests for the resource catalog.

use std::collections::{BTreeMap, HashSet};

use crate::catalog::ResourceCatalog;
use crate::drift::CatalogSnapshot;
use crate::entry::ResourceEntry;
use crate::error::ResourceError;
use crate::kind::{DbKind, DocKind, ResourceAddress, ResourceKind};
use crate::registry::ResourceRegistry;
use crate::resolve::IcebergIdent;

fn ld_entry() -> ResourceEntry {
    ResourceEntry::new(
        "ldscore.1000g_eur",
        ResourceKind::IcebergTable,
        "1000G EUR LD scores",
        ResourceAddress::iceberg("ld_score", "1000g_eur"),
    )
}

fn endpoint_entry() -> ResourceEntry {
    ResourceEntry::new(
        "endpoint.opentargets",
        ResourceKind::Endpoint,
        "OpenTargets GraphQL",
        ResourceAddress::endpoint("https://api.platform.opentargets.org/api/v4/graphql"),
    )
}

fn path_entry() -> ResourceEntry {
    ResourceEntry::new(
        "plink.1000g_eur.ref_prefix",
        ResourceKind::FilePath,
        "1000G EUR PLINK ref prefix",
        ResourceAddress::path("/mnt/disk2/dataset/1000g_plink/eur/chr{N}/1000G.EUR.chr{N}.qc"),
    )
}

#[test]
fn register_get_list_and_kind_index() {
    let mut reg = ResourceRegistry::new();
    reg.register(ld_entry()).unwrap();
    reg.register(endpoint_entry()).unwrap();

    assert_eq!(reg.len(), 2);
    assert!(reg.get("ldscore.1000g_eur").is_some());
    assert!(reg.get("nope").is_none());

    let iceberg: Vec<&ResourceEntry> = reg.list_by_kind(ResourceKind::IcebergTable).collect();
    assert_eq!(iceberg.len(), 1);
    assert_eq!(iceberg[0].name, "ldscore.1000g_eur");
}

#[test]
fn duplicate_same_address_is_idempotent() {
    let mut reg = ResourceRegistry::new();
    reg.register(ld_entry()).unwrap();
    // identical address -> no-op, no error
    reg.register(ld_entry()).unwrap();
    assert_eq!(reg.len(), 1);
}

#[test]
fn duplicate_conflicting_address_is_error() {
    let mut reg = ResourceRegistry::new();
    reg.register(ld_entry()).unwrap();
    let conflicting = ResourceEntry::new(
        "ldscore.1000g_eur",
        ResourceKind::IcebergTable,
        "changed",
        ResourceAddress::iceberg("ld_score", "ukbb_eur"),
    );
    assert!(matches!(
        reg.register(conflicting),
        Err(ResourceError::Duplicate(_))
    ));
    assert_eq!(reg.len(), 1);
}

#[test]
fn unknown_resolve_is_error() {
    let cat = ResourceCatalog::new("/tmp");
    assert!(matches!(
        cat.resolve_iceberg("nope"),
        Err(ResourceError::UnknownResource(_))
    ));
}

#[test]
fn kind_mismatch_is_error() {
    let cat = ResourceCatalog::new("/tmp");
    cat.register(endpoint_entry()).unwrap();
    assert!(matches!(
        cat.resolve_iceberg("endpoint.opentargets"),
        Err(ResourceError::KindMismatch { .. })
    ));
}

#[test]
fn validation_rejects_bad_endpoint_and_empty_table() {
    let mut reg = ResourceRegistry::new();
    let bad_url = ResourceEntry::new(
        "endpoint.bad",
        ResourceKind::Endpoint,
        "bad",
        ResourceAddress::endpoint("ftp://nope"),
    );
    assert!(matches!(reg.register(bad_url), Err(ResourceError::Validation(_))));

    let empty_table = ResourceEntry::new(
        "t.empty",
        ResourceKind::IcebergTable,
        "empty",
        ResourceAddress::IcebergTable {
            catalog: "iceberg".into(),
            schema: "s".into(),
            table: String::new(),
        },
    );
    assert!(matches!(
        reg.register(empty_table),
        Err(ResourceError::Validation(_))
    ));
}

#[test]
fn sql_identifier_quoting() {
    let id = IcebergIdent {
        catalog: "iceberg".into(),
        schema: "ld_score".into(),
        table: "1000g_eur".into(),
    };
    assert_eq!(id.sql(), "\"iceberg\".\"ld_score\".\"1000g_eur\"");
    assert_eq!(id.ident(), ("iceberg".into(), "ld_score".into(), "1000g_eur".into()));
}

#[test]
fn resolve_iceberg_endpoint_path_and_template() {
    let cat = ResourceCatalog::new("/mnt");
    cat.register(ld_entry()).unwrap();
    cat.register(endpoint_entry()).unwrap();
    cat.register(path_entry()).unwrap();

    let id = cat.resolve_iceberg("ldscore.1000g_eur").unwrap();
    assert_eq!(id.sql(), "\"iceberg\".\"ld_score\".\"1000g_eur\"");

    let url = cat.resolve_endpoint("endpoint.opentargets").unwrap();
    assert_eq!(url, "https://api.platform.opentargets.org/api/v4/graphql");

    // absolute path -> unchanged
    let p = cat.resolve_path("plink.1000g_eur.ref_prefix").unwrap();
    assert!(p.starts_with("/mnt/disk2/dataset/1000g_plink"));

    // template substitution
    let mut subs = BTreeMap::new();
    subs.insert("N".into(), "1".into());
    let resolved = cat
        .resolve_path_template("plink.1000g_eur.ref_prefix", &subs)
        .unwrap();
    assert_eq!(
        resolved,
        std::path::PathBuf::from("/mnt/disk2/dataset/1000g_plink/eur/chr1/1000G.EUR.chr1.qc")
    );
}

#[test]
fn relative_path_is_absolutized_against_base_dir() {
    let cat = ResourceCatalog::new("/base/dir");
    cat.register(ResourceEntry::new(
        "kms.db",
        ResourceKind::Database,
        "kms db",
        ResourceAddress::database(DbKind::Sqlite, "data/kms.db"),
    ))
    .unwrap();
    let p = cat.resolve_database("kms.db").unwrap();
    assert_eq!(p, std::path::PathBuf::from("/base/dir/data/kms.db"));
}

#[test]
fn drift_warns_on_missing_table_and_path_without_mutating() {
    let cat = ResourceCatalog::new("/tmp");
    cat.register(ld_entry()).unwrap();
    cat.register(path_entry()).unwrap();
    cat.register(ResourceEntry::new(
        "doc.notes",
        ResourceKind::Doc,
        "notes",
        ResourceAddress::doc(DocKind::Notes, "/tmp/does-not-exist.md"),
    ))
    .unwrap();

    // live catalog does not contain ld_score.1000g_eur
    let snapshot = CatalogSnapshot {
        tables: HashSet::new(),
    };
    let warnings = cat.check_drift(&snapshot);

    let names: Vec<&str> = warnings.iter().map(|w| w.name.as_str()).collect();
    assert!(names.contains(&"ldscore.1000g_eur"));
    assert!(names.contains(&"doc.notes"));
    // the file path is under /tmp which may exist, so only assert ld + doc.
    // The index is unchanged.
    assert_eq!(cat.list().len(), 3);
    assert!(cat.get("ldscore.1000g_eur").is_some());
}

#[test]
fn serialization_round_trip() {
    let entry = ld_entry()
        .with_metadata(BTreeMap::from([("pop".into(), "EUR".into())]))
        .with_tags(vec!["ldsc".into()]);
    let json = serde_json::to_string(&entry).unwrap();
    let back: ResourceEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(back, entry);
}
