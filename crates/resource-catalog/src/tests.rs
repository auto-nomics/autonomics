//! Unit tests for the resource catalog.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::catalog::ResourceCatalog;
use crate::entry::ResourceEntry;
use crate::error::ResourceError;
use crate::kind::{DataFormat, DbKind, ResourceAddress, ResourceKind};
use crate::registry::ResourceRegistry;
use crate::storage::StorageConfig;

// ── Test fixtures ───────────────────────────────────────────────────────

fn storage_entry() -> ResourceEntry {
    ResourceEntry::new(
        "ldscore.1000g_eur",
        ResourceKind::Storage,
        "1000G EUR LD scores",
        ResourceAddress::storage("default", "ld_score/1000g_eur/"),
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

fn config_entry() -> ResourceEntry {
    ResourceEntry::new(
        "config.aws_region",
        ResourceKind::Config,
        "AWS region",
        ResourceAddress::config("AWS_REGION", "us-east-1"),
    )
}

fn database_entry() -> ResourceEntry {
    ResourceEntry::new(
        "kms.db",
        ResourceKind::Database,
        "KMS database",
        ResourceAddress::database(DbKind::Sqlite, "data/kms.db"),
    )
}

fn cat_with_local_backend() -> ResourceCatalog {
    let cat = ResourceCatalog::new("/tmp");
    cat.register_backend("default", StorageConfig::local("/tmp"))
        .unwrap();
    cat
}

// ── Registry tests ──────────────────────────────────────────────────────

#[test]
fn register_get_list_and_kind_index() {
    let mut reg = ResourceRegistry::new();
    reg.register(storage_entry()).unwrap();
    reg.register(endpoint_entry()).unwrap();

    assert_eq!(reg.len(), 2);
    assert!(reg.get("ldscore.1000g_eur").is_some());
    assert!(reg.get("nope").is_none());

    let storage: Vec<&ResourceEntry> = reg.list_by_kind(ResourceKind::Storage).collect();
    assert_eq!(storage.len(), 1);
    assert_eq!(storage[0].name, "ldscore.1000g_eur");
}

#[test]
fn duplicate_same_address_is_idempotent() {
    let mut reg = ResourceRegistry::new();
    reg.register(storage_entry()).unwrap();
    reg.register(storage_entry()).unwrap();
    assert_eq!(reg.len(), 1);
}

#[test]
fn duplicate_conflicting_address_is_error() {
    let mut reg = ResourceRegistry::new();
    reg.register(storage_entry()).unwrap();
    let conflicting = ResourceEntry::new(
        "ldscore.1000g_eur",
        ResourceKind::Storage,
        "changed",
        ResourceAddress::storage("default", "ld_score/ukbb_eur/"),
    );
    assert!(matches!(
        reg.register(conflicting),
        Err(ResourceError::Duplicate(_))
    ));
    assert_eq!(reg.len(), 1);
}

// ── Validation tests ───────────────────────────────────────────────────

#[test]
fn validation_rejects_bad_endpoint() {
    let mut reg = ResourceRegistry::new();
    let bad_url = ResourceEntry::new(
        "endpoint.bad",
        ResourceKind::Endpoint,
        "bad",
        ResourceAddress::endpoint("ftp://nope"),
    );
    assert!(matches!(
        reg.register(bad_url),
        Err(ResourceError::Validation(_))
    ));
}

#[test]
fn validation_rejects_empty_backend() {
    let mut reg = ResourceRegistry::new();
    let bad = ResourceEntry::new(
        "x",
        ResourceKind::Storage,
        "no backend",
        ResourceAddress::Storage {
            backend: String::new(),
            path: "/foo/".into(),
            format: DataFormat::Parquet,
            partition_columns: vec![],
        },
    );
    assert!(matches!(
        reg.register(bad),
        Err(ResourceError::Validation(_))
    ));
}

#[test]
fn validation_rejects_empty_path() {
    let mut reg = ResourceRegistry::new();
    let bad = ResourceEntry::new(
        "x",
        ResourceKind::Storage,
        "no path",
        ResourceAddress::Storage {
            backend: "default".into(),
            path: String::new(),
            format: DataFormat::Parquet,
            partition_columns: vec![],
        },
    );
    assert!(matches!(
        reg.register(bad),
        Err(ResourceError::Validation(_))
    ));
}

// ── Resolution tests ───────────────────────────────────────────────────

#[test]
fn resolve_storage_returns_operator_and_path() {
    let cat = cat_with_local_backend();
    cat.register(storage_entry()).unwrap();

    let sref = cat.resolve_storage("ldscore.1000g_eur").unwrap();
    assert_eq!(sref.path, "ld_score/1000g_eur/");
}

#[test]
fn resolve_storage_unknown_resource_is_error() {
    let cat = cat_with_local_backend();
    assert!(matches!(
        cat.resolve_storage("nope"),
        Err(ResourceError::UnknownResource(_))
    ));
}

#[test]
fn resolve_storage_unregistered_backend_is_error() {
    let cat = ResourceCatalog::new("/tmp");
    // Register entry but NOT the backend
    cat.register(storage_entry()).unwrap();
    assert!(matches!(
        cat.resolve_storage("ldscore.1000g_eur"),
        Err(ResourceError::UnknownBackend(_))
    ));
}

#[test]
fn resolve_storage_kind_mismatch() {
    let cat = cat_with_local_backend();
    cat.register(endpoint_entry()).unwrap();
    assert!(matches!(
        cat.resolve_storage("endpoint.opentargets"),
        Err(ResourceError::KindMismatch { .. })
    ));
}

#[test]
fn resolve_endpoint_and_config() {
    let cat = cat_with_local_backend();
    cat.register(endpoint_entry()).unwrap();
    cat.register(config_entry()).unwrap();

    let url = cat.resolve_endpoint("endpoint.opentargets").unwrap();
    assert_eq!(url, "https://api.platform.opentargets.org/api/v4/graphql");

    let val = cat.resolve_config("config.aws_region").unwrap();
    assert_eq!(val, "us-east-1");
}

#[test]
fn resolve_database_absolutizes_sqlite() {
    let cat = ResourceCatalog::new("/base/dir");
    cat.register(database_entry()).unwrap();
    let p = cat.resolve_database("kms.db").unwrap();
    assert_eq!(p, std::path::PathBuf::from("/base/dir/data/kms.db"));
}

#[test]
fn resolve_database_passes_through_url_for_turso() {
    let cat = ResourceCatalog::new("/base/dir");
    cat.register(ResourceEntry::new(
        "turso.db",
        ResourceKind::Database,
        "turso",
        ResourceAddress::database(DbKind::Turso, "https://db.example.com"),
    ))
    .unwrap();
    let p = cat.resolve_database("turso.db").unwrap();
    assert_eq!(p, std::path::PathBuf::from("https://db.example.com"));
}

// ── Storage template ──────────────────────────────────────────────────

#[test]
fn resolve_storage_template_substitutes_placeholders() {
    let cat = cat_with_local_backend();
    cat.register(ResourceEntry::new(
        "plink.1000g_eur.ref_prefix",
        ResourceKind::Storage,
        "1000G EUR PLINK ref prefix",
        ResourceAddress::storage("default", "1000g_plink/eur/chr{N}/1000G.EUR.chr{N}.qc"),
    ))
    .unwrap();

    let mut subs = BTreeMap::new();
    subs.insert("N".into(), "1".into());
    let sref = cat
        .resolve_storage_template("plink.1000g_eur.ref_prefix", &subs)
        .unwrap();
    assert_eq!(
        sref.path,
        "1000g_plink/eur/chr1/1000G.EUR.chr1.qc"
    );
}

#[test]
fn resolve_storage_template_unresolved_placeholder_is_error() {
    let cat = cat_with_local_backend();
    cat.register(ResourceEntry::new(
        "tmpl",
        ResourceKind::Storage,
        "template",
        ResourceAddress::storage("default", "data/{N}/file"),
    ))
    .unwrap();

    let subs = BTreeMap::new(); // no substitution for {N}
    assert!(matches!(
        cat.resolve_storage_template("tmpl", &subs),
        Err(ResourceError::Validation(_))
    ));
}

#[test]
fn resolve_storage_url_builds_datafusion_listing_urls() {
    let cat = ResourceCatalog::new("/tmp");
    cat.register_backend("default", StorageConfig::local("/tmp"))
        .unwrap();
    cat.register_backend(
        "s3-prod",
        StorageConfig::s3("my-bucket", "us-east-1", "ak", "sk"),
    )
    .unwrap();
    cat.register(storage_entry()).unwrap();
    cat.register(ResourceEntry::new(
        "ldscore.s3",
        ResourceKind::Storage,
        "S3 panel",
        ResourceAddress::storage("s3-prod", "ld_score/1000g_eur/"),
    ))
    .unwrap();

    assert_eq!(
        cat.resolve_storage_url("ldscore.1000g_eur").unwrap(),
        "file:///ld_score/1000g_eur/"
    );
    assert_eq!(
        cat.resolve_storage_url("ldscore.s3").unwrap(),
        "s3://my-bucket/ld_score/1000g_eur/"
    );
}

// ── Serialization ──────────────────────────────────────────────────────

#[test]
fn serialization_round_trip() {
    let entry = storage_entry()
        .with_metadata(BTreeMap::from([("pop".into(), "EUR".into())]))
        .with_tags(vec!["ldsc".into()]);
    let json = serde_json::to_string(&entry).unwrap();
    let back: ResourceEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(back, entry);
}

#[test]
fn storage_with_partition_columns_round_trips() {
    let entry = ResourceEntry::new(
        "ld_matrix.eur",
        ResourceKind::Storage,
        "chr-partitioned LD matrix",
        ResourceAddress::storage_with(
            "default",
            "ld_matrix/eur/",
            DataFormat::Parquet,
            vec!["chr".into()],
        ),
    );
    let json = serde_json::to_string(&entry).unwrap();
    let back: ResourceEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(back, entry);
    match &back.address {
        ResourceAddress::Storage { partition_columns, .. } => {
            assert_eq!(partition_columns, &vec!["chr".to_string()]);
        }
        _ => panic!("expected Storage"),
    }
}

#[test]
fn storage_config_round_trips() {
    let configs = vec![
        StorageConfig::local("/data/buckets"),
        StorageConfig::s3("my-bucket", "us-east-1", "AKID", "SECRET"),
        StorageConfig::s3_compatible(
            "datalake",
            "http://localhost:3900",
            "garage",
            "AKID",
            "SECRET",
        ),
        StorageConfig::oss(
            "lance-db",
            "oss-cn-beijing.aliyuncs.com",
            "AKID",
            "SECRET",
        ),
    ];
    for cfg in configs {
        let json = serde_json::to_string(&cfg).unwrap();
        let back: StorageConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }
}

// ── Drift ───────────────────────────────────────────────────────────────

#[test]
fn drift_warns_on_missing_local_storage() {
    let cat = ResourceCatalog::new("/tmp");
    cat.register_backend("default", StorageConfig::local("/tmp"))
        .unwrap();
    cat.register(storage_entry()).unwrap();
    cat.register(database_entry()).unwrap();

    let snapshot = crate::drift::CatalogSnapshot::default();
    let warnings = cat.check_drift(&snapshot);

    let names: Vec<&str> = warnings.iter().map(|w| w.name.as_str()).collect();
    assert!(names.contains(&"ldscore.1000g_eur"));
    assert_eq!(cat.list().len(), 2); // index unchanged
}

// ── Backend management ─────────────────────────────────────────────────

#[test]
fn register_and_list_backends() {
    let cat = ResourceCatalog::new("/tmp");
    cat.register_backend("local", StorageConfig::local("/tmp"))
        .unwrap();
    cat.register_backend("prod", StorageConfig::s3("bucket", "region", "ak", "sk"))
        .unwrap();

    let names = cat.backend_names();
    assert!(names.contains(&"local".to_string()));
    assert!(names.contains(&"prod".to_string()));
}

// ── Persist round-trip ─────────────────────────────────────────────────

#[tokio::test]
async fn persist_round_trip_with_storage_entry() {
    use crate::persist::TursoManifestStore;

    let store: Arc<dyn crate::persist::ManifestStore> =
        Arc::new(TursoManifestStore::open_in_memory().await.unwrap());
    let cat = ResourceCatalog::with_persist("/tmp", store.clone());
    cat.register(storage_entry()).unwrap();
    cat.register(endpoint_entry()).unwrap();
    cat.persist().await;

    let loaded = store.load().await.unwrap();
    assert_eq!(loaded.len(), 2);
    let names: Vec<&str> = loaded.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"ldscore.1000g_eur"));
    assert!(names.contains(&"endpoint.opentargets"));
}
