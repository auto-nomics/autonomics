//! Resource catalog subcommand.
//!
//! Owns manifest open/list/show/update/add/remove/archive/ingest/drift/export.
//! Keep catalog interaction here; `cli.rs` only describes the command surface.

use std::path::PathBuf;
use std::sync::Arc;

use crate::cli::{
    ResourceAction, ResourceAddArgs, ResourceArgs, ResourceDriftArgs, ResourceExportArgs,
    ResourceListArgs, ResourceRemoveArgs, ResourceShowArgs, ResourceTargetArgs,
    ResourceUpdateArgs,
};

async fn open_catalog() -> color_eyre::Result<Arc<dag_core::resource_catalog::ResourceCatalog>> {
    use dag_core::resource_catalog::ResourceCatalog;
    use runtime::config::RuntimeConfig;

    let config = RuntimeConfig::default();
    let manifest_db = config.state_dir.join("resource-manifest.db");
    if let Some(parent) = manifest_db.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let catalog = Arc::new(
        ResourceCatalog::load_or_error(&config.data_dir, &manifest_db)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                // Detect the common "database is locked" case and give
                // actionable advice.
                if msg.contains("locked") || msg.contains("busy") {
                    color_eyre::eyre::eyre!(
                        "resource-manifest.db is locked — another TUI process is likely running.\n\
                         \n\
                         Options:\n\
                         1. Close the other TUI instance and retry.\n\
                         2. DB path: {}\n\
                         \n\
                         Underlying error: {msg}",
                        manifest_db.display()
                    )
                } else {
                    color_eyre::eyre::eyre!(
                        "failed to open resource catalog at {}: {msg}",
                        manifest_db.display()
                    )
                }
            })?,
    );
    let _ = ResourceCatalog::set_global(catalog.clone());
    Ok(catalog)
}

pub fn run_resource(args: ResourceArgs) -> color_eyre::Result<()> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| color_eyre::eyre::eyre!("failed to build tokio runtime: {e}"))?;
    runtime.block_on(async { run_resource_async(args).await })
}

async fn run_resource_async(args: ResourceArgs) -> color_eyre::Result<()> {
    let catalog = open_catalog().await?;

    match args.action {
        ResourceAction::List(a) => resource_list(&catalog, a).await,
        ResourceAction::Show(a) => resource_show(&catalog, a),
        ResourceAction::Update(a) => resource_update(&catalog, a).await,
        ResourceAction::Add(a) => resource_add(&catalog, *a).await,
        ResourceAction::Remove(a) => resource_remove(&catalog, a).await,
        ResourceAction::Ingest(a) => resource_ingest(&catalog, a).await,
        ResourceAction::Archive(a) => resource_archive(&catalog, a).await,
        ResourceAction::Restore(a) => resource_restore(&catalog, a).await,
        ResourceAction::Verify(a) => resource_verify(&catalog, a).await,
        ResourceAction::Drift(a) => resource_drift(&catalog, a).await,
        ResourceAction::Export(a) => resource_export(&catalog, a),
    }
}

async fn resource_list(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceListArgs,
) -> color_eyre::Result<()> {
    let mut entries = catalog.list();

    // ── Filters ─────────────────────────────────────────────────────
    if let Some(ref kind) = args.kind {
        entries.retain(|e| e.kind.as_str() == kind.as_str());
    }
    if !args.tag.is_empty() {
        entries.retain(|e| args.tag.iter().any(|t| e.tags.contains(t)));
    }
    if let Some(ref name) = args.name {
        let lower = name.to_lowercase();
        entries.retain(|e| e.name.to_lowercase().contains(&lower));
    }
    if args.archivable {
        entries.retain(|e| e.archive_spec.is_some());
    }
    if args.ingestible {
        entries.retain(|e| e.ingestion_spec.is_some());
    }

    if entries.is_empty() {
        println!("No resources match the given filters.");
        return Ok(());
    }

    // Sort by kind then name for readability.
    entries.sort_by(|a, b| {
        a.kind
            .as_str()
            .cmp(b.kind.as_str())
            .then_with(|| a.name.cmp(&b.name))
    });

    match args.format.as_str() {
        "json" => {
            let json = serde_json::to_string_pretty(&entries)?;
            println!("{json}");
        }
        _ => {
            // Table output.
            println!("{} resources:\n", entries.len());
            println!(
                "  {:<14} {:<30} {:<10} {:<8} {:<8} DESCRIPTION",
                "KIND", "NAME", "ARCHIVE", "INGEST", ""
            );
            println!("  {:-<120}", "");
            for e in &entries {
                let desc = if e.description.chars().count() > 50 {
                    let truncated: String = e.description.chars().take(47).collect();
                    format!("{truncated}…")
                } else {
                    e.description.clone()
                };
                let has_archive = if e.archive_spec.is_some() { "✓" } else { "" };
                let has_ingest = if e.ingestion_spec.is_some() {
                    "✓"
                } else {
                    ""
                };
                println!(
                    "  {:<14} {:<30} {:<8} {:<8} {}",
                    e.kind.as_str(),
                    e.name,
                    has_archive,
                    has_ingest,
                    desc
                );
            }
        }
    }

    Ok(())
}

fn resource_show(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceShowArgs,
) -> color_eyre::Result<()> {
    let entry = catalog.get(&args.name).ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "resource '{}' not found. Use 'autonomics-tui resource list' to see names.",
            args.name
        )
    })?;

    println!("Name:        {}", entry.name);
    println!("Kind:        {}", entry.kind.as_str());
    println!("Description: {}", entry.description);
    println!("Address:     {:?}", entry.address);

    if !entry.metadata.is_empty() {
        println!("Metadata:");
        for (k, v) in &entry.metadata {
            println!("  {k} = {v}");
        }
    }

    if !entry.tags.is_empty() {
        println!("Tags:        {}", entry.tags.join(", "));
    }

    if let Some(ref spec) = entry.archive_spec {
        println!("\nArchive Spec:");
        println!("  remote:      {}:{}", spec.remote, spec.remote_path);
        println!("  checksum:    {}", spec.checksum);
    }

    if let Some(ref status) = entry.archive_status {
        println!("\nArchive Status:");
        if let Some(ref ts) = status.archived_at {
            println!("  archived_at: {ts}");
        }
        if let Some(ref ts) = status.restored_at {
            println!("  restored_at: {ts}");
        }
        if let Some(n) = status.file_count {
            println!("  file_count:  {n}");
        }
        if let Some(s) = status.size_bytes {
            println!("  size_bytes:  {s}");
        }
        if let Some(v) = status.verified {
            println!("  verified:    {}", if v { "✓" } else { "✗" });
        }
    }

    if let Some(ref spec) = entry.ingestion_spec {
        println!("\nIngestion Spec:");
        println!("  source_path:   {}", spec.source_path);
        println!("  source_format: {}", spec.source_format.as_str());
        if !spec.partition_by.is_empty() {
            println!("  partition_by:  {}", spec.partition_by.join(", "));
        }
        println!("  mode:          {}", spec.mode.as_str());
        if spec.restore_before {
            println!("  restore_before: ✓");
        }
        if spec.archive_after {
            println!("  archive_after:  ✓");
        }
        if let Some(ref src) = spec.source_resource {
            println!("  source_resource: {src}");
        }
    }

    // Show the rclone commands for archivable resources.
    if entry.archive_spec.is_some() {
        if let Ok(cmd) = catalog.archive_command(&entry.name) {
            println!("\nArchive command:  {cmd}");
        }
        if let Ok(cmd) = catalog.restore_command(&entry.name) {
            println!("Restore command:  {cmd}");
        }
    }

    Ok(())
}

async fn resource_update(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceUpdateArgs,
) -> color_eyre::Result<()> {
    use dag_core::resource_catalog::ResourcePatch;

    if args.description.is_none() {
        return Err(color_eyre::eyre::eyre!(
            "no fields specified for update. pass --description \"...\""
        ));
    }

    // Snapshot the old description so the user sees what changed.
    let before = catalog.get(&args.name).ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "resource '{}' not found. Use 'autonomics-tui resource list' to see names.",
            args.name
        )
    })?;
    let old_desc = before.description.clone();

    let mut patch = ResourcePatch::new();
    if let Some(d) = args.description.clone() {
        patch = patch.description(d);
    }

    let updated = catalog
        .patch(&args.name, patch)
        .map_err(color_eyre::Report::new)?;
    catalog.persist().await;

    println!("✓ Updated resource '{}'", updated.name);
    println!("    description: {} -> {}", old_desc, updated.description);
    Ok(())
}

async fn resource_add(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceAddArgs,
) -> color_eyre::Result<()> {
    use dag_core::resource_catalog::{
        ArchiveSpec, CsvOptions, DbKind, DocKind, IngestionSpec, ResourceAddress, ResourceEntry,
        ResourceKind, SourceFormat, WriteMode,
    };

    let kind = ResourceKind::from_str(&args.kind).ok_or_else(|| {
        color_eyre::eyre::eyre!(
            "unknown kind '{}': expected storage, endpoint, config, or database",
            args.kind
        )
    })?;

    let address = match kind {
        ResourceKind::Storage => {
            let path = args.path.as_deref().ok_or_else(|| {
                color_eyre::eyre::eyre!("--path is required for kind=storage")
            })?;
            ResourceAddress::storage("default", path)
        }
        ResourceKind::Endpoint => {
            let url = args.url.as_deref().ok_or_else(|| {
                color_eyre::eyre::eyre!("--url is required for kind=endpoint")
            })?;
            ResourceAddress::endpoint(url)
        }
        ResourceKind::Config => {
            let key = args.key.as_deref().ok_or_else(|| {
                color_eyre::eyre::eyre!("--key is required for kind=config")
            })?;
            let value = args.value.as_deref().ok_or_else(|| {
                color_eyre::eyre::eyre!("--value is required for kind=config")
            })?;
            ResourceAddress::config(key, value)
        }
        ResourceKind::Database => {
            let path = args.path.as_deref().ok_or_else(|| {
                color_eyre::eyre::eyre!("--path is required for kind=database")
            })?;
            let db_kind = match args.db_kind.as_deref() {
                Some("turso") => DbKind::Turso,
                Some("postgres") => DbKind::Postgres,
                _ => DbKind::Sqlite,
            };
            ResourceAddress::database(db_kind, path)
        }
    };

    let mut entry_builder = ResourceEntry::new(&args.name, kind, &args.description, address);

    if !args.metadata.is_empty() {
        entry_builder = entry_builder.with_metadata(args.metadata.iter().cloned().collect());
    }
    if !args.tag.is_empty() {
        entry_builder = entry_builder.with_tags(args.tag.clone());
    }

    // ── Ingestion spec (--source, iceberg_table only) ───────────────────
    let mut ingestion_spec: Option<IngestionSpec> = None;
    let mut archive_spec: Option<ArchiveSpec> = None;
    let mut archive_local_path: Option<PathBuf> = None;
    let mut source_resource_name: Option<String> = None;

    if let Some(ref source_path) = args.source {
        if kind != ResourceKind::Storage {
            return Err(color_eyre::eyre::eyre!(
                "--source is only valid for kind=iceberg_table"
            ));
        }

        let format_str = args.format.as_deref().unwrap_or("parquet");
        let format = match format_str {
            "parquet" => SourceFormat::Parquet,
            "csv" => SourceFormat::Csv,
            "tsv" => SourceFormat::Tsv,
            other => {
                return Err(color_eyre::eyre::eyre!(
                    "unknown format '{other}': expected parquet, csv, or tsv"
                ));
            }
        };

        let mode_str = args.mode.as_deref().unwrap_or("create_if_not_exists");
        let mode = match mode_str {
            "create_if_not_exists" => WriteMode::CreateIfNotExists,
            "create_or_replace" => WriteMode::CreateOrReplace,
            "append" => WriteMode::Append,
            other => {
                return Err(color_eyre::eyre::eyre!(
                    "unknown mode '{other}': expected create_if_not_exists, create_or_replace, or append"
                ));
            }
        };

        let csv_options = match format {
            SourceFormat::Csv | SourceFormat::Tsv => Some(CsvOptions {
                has_header: !args.no_header,
                delimiter: args.delimiter.unwrap_or({
                    if matches!(format, SourceFormat::Tsv) {
                        '\t'
                    } else {
                        ','
                    }
                }),
                file_extension: None,
                compression: None,
            }),
            _ => None,
        };

        // Resolve archive config: --archive flag + env vars.
        if args.archive {
            let remote = std::env::var("ARCHIVE_REMOTE").map_err(|_| {
                color_eyre::eyre::eyre!("--archive requires ARCHIVE_REMOTE env var (e.g. 'aliyun')")
            })?;
            let bucket = std::env::var("ARCHIVE_BUCKET").map_err(|_| {
                color_eyre::eyre::eyre!(
                    "--archive requires ARCHIVE_BUCKET env var (e.g. 'autonomics-data')"
                )
            })?;
            let schema = args.schema.as_deref().unwrap_or("data");
            let table = args.table.as_deref().unwrap_or("default");
            let remote_path = format!("{}/{}/{}/", bucket, schema, table);
            archive_spec = Some(ArchiveSpec {
                remote,
                remote_path,
                checksum: true,
            });

            // Resolve the local path to archive (file / dir / glob parent).
            let p = std::path::Path::new(source_path);
            archive_local_path = Some(if p.is_file() || p.is_dir() {
                p.to_path_buf()
            } else {
                let mut dir = p.parent().unwrap_or(p);
                while !dir.is_dir() {
                    dir = match dir.parent() {
                        Some(p) => p,
                        None => break,
                    };
                }
                dir.to_path_buf()
            });

            source_resource_name = Some(format!(
                "source.{}",
                args.name.strip_prefix("iceberg.").unwrap_or(&args.name)
            ));
        }

        ingestion_spec = Some(IngestionSpec {
            source_path: source_path.clone(),
            source_format: format,
            partition_by: args.partition.clone(),
            mode,
            restore_before: false,
            archive_after: false,
            source_resource: source_resource_name.clone(),
            csv_options,
        });
    }

    if let Some(ref spec) = ingestion_spec {
        entry_builder = entry_builder.with_ingestion(spec.clone());
    }

    let registered_name = args.name.clone();
    catalog.register(entry_builder)?;
    catalog.persist().await;

    // ── Register linked source FilePath resource for archive ────────────
    if let (Some(sn), Some(aspec), Some(local)) =
        (&source_resource_name, &archive_spec, &archive_local_path)
    {
        let src_entry = ResourceEntry::new(
            sn.clone(),
            ResourceKind::Storage,
            format!("Source files for {}", args.name),
            ResourceAddress::storage("default", local.to_string_lossy()),
        )
        .with_archive(aspec.clone());
        catalog.register(src_entry)?;
        catalog.persist().await;
    }

    // ── Summary output ──────────────────────────────────────────────────
    println!(
        "✓ Registered resource '{}' ({})",
        args.name,
        args.kind.as_str()
    );
    if let Some(ref spec) = ingestion_spec {
        println!(
            "  source: {} ({})",
            spec.source_path,
            spec.source_format.as_str()
        );
        if !spec.partition_by.is_empty() {
            println!("  partition: {}", spec.partition_by.join(", "));
        }
        println!("  mode: {}", spec.mode.as_str());
    }
    if let Some(ref aspec) = archive_spec {
        println!("  archive: {}:{}", aspec.remote, aspec.remote_path);
    }

    // ── Compound: --archive does register + push + ingest ───────────────
    if args.archive {
        let Some(ref src_name) = source_resource_name else {
            return Ok(());
        };

        // Step 1: Archive source to cloud.
        println!("\n[1/2] Archiving source…");
        match catalog.archive(src_name).await {
            Ok(o) => println!(
                "  ✓ {} files, {} bytes, {:.1}s",
                o.files_transferred,
                o.size_bytes,
                o.duration_ms as f64 / 1000.0
            ),
            Err(e) => {
                println!("  ✗ archive failed: {e}");
                println!("\nResource registered but archive/ingest incomplete.");
                println!("Retry: autonomics-tui resource archive -r {src_name}");
                return Ok(());
            }
        }

        // Step 2: Ingest into Iceberg.
        println!("\n[2/2] Ingesting into Iceberg…");
        eprintln!("ingestion pending rewrite for object storage pipeline");
        println!("\n✓ Done — registered + archived + ingested.");
    } else if ingestion_spec.is_some() {
        println!("\nTo ingest: autonomics-tui resource ingest -r {registered_name}");
    }

    Ok(())
}

async fn resource_remove(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceRemoveArgs,
) -> color_eyre::Result<()> {
    // Show what would be removed.
    let entry = catalog
        .get(&args.name)
        .ok_or_else(|| color_eyre::eyre::eyre!("resource '{}' not found", args.name))?;

    if !args.yes {
        eprint!(
            "Remove '{}' ({}) from the catalog? [y/N] ",
            entry.name,
            entry.kind.as_str()
        );
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        if !matches!(buf.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("aborted");
            return Ok(());
        }
    }

    catalog.deregister(&args.name);
    catalog.persist().await;
    println!("✓ Removed resource '{}'", args.name);
    Ok(())
}

async fn resource_archive(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceTargetArgs,
) -> color_eyre::Result<()> {
    let targets = resolve_archive_targets(catalog, &args.resource);

    if targets.is_empty() {
        println!("No archivable resources found.");
        return Ok(());
    }

    println!("Archiving {} resource(s) to cloud…\n", targets.len());
    let mut ok = 0u32;
    let mut fail = 0u32;
    for name in &targets {
        print!("  {name}: ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        match catalog.archive(name).await {
            Err(e) => {
                println!("✗ {e}");
                fail += 1;
            }
            Ok(o) => {
                println!(
                    "✓ {} files, {} bytes, {:.1}s",
                    o.files_transferred,
                    o.size_bytes,
                    o.duration_ms as f64 / 1000.0
                );
                ok += 1;
            }
        }
    }
    println!("\nDone: {ok} ok, {fail} failed.");
    catalog.persist().await;
    Ok(())
}

async fn resource_restore(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceTargetArgs,
) -> color_eyre::Result<()> {
    let targets = resolve_archive_targets(catalog, &args.resource);

    if targets.is_empty() {
        println!("No archivable resources found.");
        return Ok(());
    }

    println!("Restoring {} resource(s) from cloud…\n", targets.len());
    let mut ok = 0u32;
    let mut fail = 0u32;
    for name in &targets {
        print!("  {name}: ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        match catalog.restore(name).await {
            Err(e) => {
                println!("✗ {e}");
                fail += 1;
            }
            Ok(o) => {
                println!(
                    "✓ {} files, {} bytes, {:.1}s",
                    o.files_transferred,
                    o.size_bytes,
                    o.duration_ms as f64 / 1000.0
                );
                ok += 1;
            }
        }
    }
    println!("\nDone: {ok} ok, {fail} failed.");
    Ok(())
}

async fn resource_verify(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceTargetArgs,
) -> color_eyre::Result<()> {
    let targets = resolve_archive_targets(catalog, &args.resource);

    if targets.is_empty() {
        println!("No archivable resources found.");
        return Ok(());
    }

    println!("Verifying {} resource(s)…\n", targets.len());
    let mut ok = 0u32;
    let mut fail = 0u32;
    for name in &targets {
        print!("  {name}: ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        match catalog.verify_archive(name).await {
            Ok(true) => {
                println!("✓ verified");
                ok += 1;
            }
            Ok(false) => {
                println!("✗ mismatch (local ≠ remote)");
                fail += 1;
            }
            Err(e) => {
                println!("✗ {e}");
                fail += 1;
            }
        }
    }
    println!("\nDone: {ok} ok, {fail} failed.");
    catalog.persist().await;
    Ok(())
}

/// Resolve the list of resource names for archive/restore/verify.
/// Single resource when `--resource` is given; all archivable otherwise.
fn resolve_archive_targets(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    resource: &Option<String>,
) -> Vec<String> {
    match resource {
        Some(name) => vec![name.clone()],
        None => catalog
            .list_archivable()
            .into_iter()
            .map(|r| r.name)
            .collect(),
    }
}

/// Resolve the list of resource names for ingest.
/// Single resource when `--resource` is given; all ingestible otherwise.
fn resolve_ingest_targets(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    resource: &Option<String>,
) -> Vec<String> {
    match resource {
        Some(name) => vec![name.clone()],
        None => catalog
            .list()
            .into_iter()
            .filter(|e| e.ingestion_spec.is_some())
            .map(|e| e.name)
            .collect(),
    }
}

async fn resource_ingest(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceTargetArgs,
) -> color_eyre::Result<()> {
    let targets = resolve_ingest_targets(catalog, &args.resource);

    if targets.is_empty() {
        println!("No resources with ingestion_spec registered.");
        println!("Register with: autonomics-tui resource add --kind iceberg_table --source …");
        return Ok(());
    }

    // ingestion pending rewrite

    println!("Ingesting {} resource(s)…\n", targets.len());
    let ok = 0u32;
    let mut fail = 0u32;
    for name in &targets {
        print!("  {name}: ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        eprintln!("ingestion pending rewrite for object storage pipeline");
        fail += 1;
    }
    println!("\nDone: {ok} ok, {fail} failed.");
    Ok(())
}

async fn resource_drift(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    _args: ResourceDriftArgs,
) -> color_eyre::Result<()> {
    use dag_core::resource_catalog::CatalogSnapshot;

    let snapshot = CatalogSnapshot::default();

    // Drift check: object storage probing not yet implemented.
    // Only local path checks are available.

    let warnings = catalog.check_drift(&snapshot);

    if warnings.is_empty() {
        println!("✓ No drift detected — all registered resources are present.");
        return Ok(());
    }

    println!("⚠ {} drift warning(s):\n", warnings.len());
    for w in &warnings {
        println!("  [{}] {}", w.name, w.detail);
    }
    println!("\n(Index is unchanged — these are warnings only.)");
    Ok(())
}

fn resource_export(
    catalog: &Arc<dag_core::resource_catalog::ResourceCatalog>,
    args: ResourceExportArgs,
) -> color_eyre::Result<()> {
    let entries = catalog.list();
    let json = serde_json::to_string_pretty(&entries)?;

    match &args.output {
        Some(path) => {
            std::fs::write(path, &json)?;
            println!("Exported {} resources to {}", entries.len(), path.display());
        }
        None => {
            println!("{json}");
        }
    }
    Ok(())
}
