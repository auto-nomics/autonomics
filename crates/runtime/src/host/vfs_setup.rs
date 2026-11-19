//! VFS assembly: the `state_dir/vfs.toml` manifest, lifecycle-managed
//! mounts (plugins, environment dev, literature, container workspace), and
//! the optional data-catalog wiring that layers panel bundles into the
//! mount table and the bundle registry.

use std::sync::Arc;

use container_runtime::PodmanConfig;
use data_catalog::{CatalogConfig, LocalCatalog, RemoteCatalog, default_panel_cache_root};
use vfs::{BackendConfig, BackendDefinition, MountDefinition, MountedObjectStore, VfsManifest};

use crate::catalog_tools::CatalogState;
use crate::config::RuntimeConfig;
use crate::error::{Error, Result};

/// Agent-visible mount for files downloaded by browser agents.
pub(super) const BROWSER_DOWNLOAD_VFS_PATH: &str = "/browser-downloads";

pub(super) fn browser_download_root(config: &RuntimeConfig) -> std::path::PathBuf {
    config
        .browser_download_dir
        .clone()
        .unwrap_or_else(|| config.state_dir.join("browser-downloads"))
}

/// Wiki attachment blobs' own VFS mount — isolated from the root workspace so
/// workspace turnover cannot delete them. `VfsBlobStore` in `open_wiki` writes
/// through the `wiki-attachments` prefix, which resolves onto this mount.
pub(super) const WIKI_ATTACHMENTS_VFS_PATH: &str = "/wiki-attachments";

pub(super) fn wiki_attachments_root(config: &RuntimeConfig) -> std::path::PathBuf {
    config.state_dir.join("wiki-attachments")
}

/// Build the bibliography VFS without opening agent or writing-system storage.
///
/// CLI commands use this to ensure they follow exactly the same
/// `state_dir/vfs.toml` mount rules as `SharedInfra`.
pub fn bibliography_file_storage(config: &RuntimeConfig) -> Result<Arc<vfs::OpendalFileStorage>> {
    let vfs = Arc::new(build_vfs(config)?);
    Ok(Arc::new(vfs::OpendalFileStorage::with_mounts(
        &config.data_dir,
        vfs.clone(),
    )))
}

/// Build the VFS from `state_dir/vfs.toml`.
///
/// On first launch the default manifest is materialized to disk so operators
/// can inspect and edit the exact mounts the process is using. The generated
/// file can contain credentials supplied by the environment, so its parent
/// directory is created and the file is written with user-only permissions on
/// Unix.
pub(super) fn build_vfs(config: &RuntimeConfig) -> Result<MountedObjectStore> {
    let state = load_or_create_vfs_manifest(config)?;
    let mut manifest = state.manifest;
    let plugin_layout = plugin_rsi::PluginStateLayout::open(&config.state_dir);
    ensure_plugin_mounts(&mut manifest, config, &plugin_layout)?;
    ensure_executor_workspace_mounts(&mut manifest, config);
    ensure_environment_dev_mount(&mut manifest, config)?;
    ensure_browser_download_mount(&mut manifest, config)?;
    MountedObjectStore::from_manifest(&manifest).map_err(|e| Error::Other(e.to_string()))
}

/// Mount every root a container executor may write outputs under.
///
/// The current workspace is independent of the Podman storage driver.
/// Historical workspace roots retain identity mounts so persisted absolute
/// FileRefs never fall through to an unrelated catch-all VFS mount.
pub(super) fn ensure_executor_workspace_mounts(manifest: &mut VfsManifest, config: &RuntimeConfig) {
    let executor = PodmanConfig::for_state_dir(&config.state_dir);
    ensure_workspace_mount(manifest, &executor.workspace_root);
    // Historical FileRefs contain absolute paths. Keep their identity mounts
    // readable without moving data or changing the active daemon's storage.
    for relative in [
        "state/work",
        "containers/workspace",
        "containers-overlay/workspace",
    ] {
        let legacy = config.state_dir.join(relative);
        if legacy.exists() {
            // Keep aliases as well as the canonical target: after an offline
            // move, persisted FileRefs can still name the old symlink leaf.
            ensure_workspace_mount(manifest, &legacy);
            if let (Some(parent), Some(name)) = (legacy.parent(), legacy.file_name()) {
                if let Ok(parent) = parent.canonicalize() {
                    ensure_workspace_mount(manifest, &parent.join(name));
                }
            }
            let root = container_runtime::canonicalize_workspace_root(legacy);
            ensure_workspace_mount(manifest, &root);
        }
    }
}

pub(super) async fn build_vfs_with_catalog(
    config: &RuntimeConfig,
) -> Result<(
    MountedObjectStore,
    dag_core::BundleRegistry,
    Option<Arc<CatalogState>>,
)> {
    let state = load_or_create_vfs_manifest(config)?;
    let mut manifest = state.manifest;
    let plugin_layout = plugin_rsi::PluginStateLayout::open(&config.state_dir);
    ensure_plugin_mounts(&mut manifest, config, &plugin_layout)?;
    ensure_executor_workspace_mounts(&mut manifest, config);
    ensure_environment_dev_mount(&mut manifest, config)?;
    ensure_browser_download_mount(&mut manifest, config)?;
    ensure_wiki_attachments_mount(&mut manifest, config)?;

    let mut catalog_registry = dag_core::BundleRegistry::new();
    let mut catalog = None;
    if let Some(catalog_source) = state.catalog_source {
        let catalog_config = CatalogConfig::from_vfs_toml(&format!(
            "[[mount]]\npath=\"/\"\nbackend=\"x\"\nsource=\"/\"\n\n{catalog_source}"
        ))
        .map_err(|error| Error::Other(error.to_string()))?;
        if catalog_config.enabled {
            const CACHE_BACKEND_ID: &str = "autonomics-catalog-cache";
            if manifest
                .backend
                .iter()
                .any(|backend| backend.id == CACHE_BACKEND_ID)
            {
                return Err(Error::Other(format!(
                    "backend `{CACHE_BACKEND_ID}` is reserved for the local catalog cache"
                )));
            }
            let repository = catalog_config
                .repository
                .as_deref()
                .ok_or_else(|| Error::Other("catalog repository is required".to_string()))?;
            let remote = RemoteCatalog::hf(
                repository,
                catalog_config.revision.clone(),
                data_catalog::hf::resolve_hf_token(None),
            )
            .map_err(|error| Error::Other(error.to_string()))?;
            let local = LocalCatalog::open(default_panel_cache_root())
                .map_err(|error| Error::Other(error.to_string()))?;
            let mounts = local
                .mount_definitions(CACHE_BACKEND_ID, catalog_config.agent_visible)
                .map_err(|error| Error::Other(error.to_string()))?;
            manifest.backend.push(BackendDefinition {
                id: CACHE_BACKEND_ID.into(),
                config: BackendConfig::local(local.root().to_string_lossy().into_owned()),
            });
            for mount in mounts {
                if manifest
                    .mount
                    .iter()
                    .any(|existing| existing.path == mount.path)
                {
                    return Err(Error::Other(format!(
                        "catalog mount path `{}` collides with a static VFS mount",
                        mount.path
                    )));
                }
                manifest.mount.push(mount);
            }
            catalog_registry = local
                .bundle_registry()
                .map_err(|error| Error::Other(error.to_string()))?;
            catalog = Some(Arc::new(CatalogState {
                local: Arc::new(local),
                remote: Arc::new(remote),
            }));
        }
    }
    MountedObjectStore::from_manifest(&manifest)
        .map_err(|e| Error::Other(e.to_string()))
        .map(|store| (store, catalog_registry, catalog))
}

fn ensure_plugin_mounts(
    manifest: &mut vfs::VfsManifest,
    _config: &RuntimeConfig,
    plugin_layout: &plugin_rsi::PluginStateLayout,
) -> Result<()> {
    const ACTIVE_BACKEND_ID: &str = "autonomics-plugin-runtime";
    const DEV_BACKEND_ID: &str = "autonomics-plugin-development";
    let registry_path = plugin_layout.registry_path();
    let Ok(text) = std::fs::read_to_string(&registry_path) else {
        return Ok(());
    };
    let parsed: plugin::sync::PluginsConfig = toml::from_str(&text)
        .map_err(|error| Error::Other(format!("parse `{}`: {error}", registry_path.display())))?;
    if manifest
        .backend
        .iter()
        .any(|backend| backend.id == ACTIVE_BACKEND_ID || backend.id == DEV_BACKEND_ID)
    {
        return Err(Error::Other(
            "plugin runtime backends are reserved for lifecycle-managed plugin mounts".to_string(),
        ));
    }

    let development_root = plugin_layout.workspace_root();
    std::fs::create_dir_all(&development_root)
        .map_err(|error| Error::Other(format!("create `{development_root:?}`: {error}")))?;
    manifest.backend.push(vfs::BackendDefinition {
        id: DEV_BACKEND_ID.into(),
        config: vfs::BackendConfig::local(development_root.to_string_lossy().into_owned()),
    });
    manifest.mount.push(vfs::MountDefinition {
        domain: None,
        path: "/plugins/dev".into(),
        backend: DEV_BACKEND_ID.into(),
        source: "/".into(),
        read_only: false,
        permissions: plugin_development_permissions(),
    });

    if !parsed.plugin.is_empty() {
        let runtime_root = plugin_layout.runtime_root();
        manifest.backend.push(vfs::BackendDefinition {
            id: ACTIVE_BACKEND_ID.into(),
            config: vfs::BackendConfig::local(runtime_root.to_string_lossy().into_owned()),
        });
        manifest.mount.push(vfs::MountDefinition {
            domain: None,
            path: "/plugins/active".into(),
            backend: ACTIVE_BACKEND_ID.into(),
            source: "/".into(),
            read_only: true,
            permissions: vfs::permission::MountPermissions::unix(
                vfs::permission::VfsOwnership::root(),
                vfs::permission::VfsMode::from_bits(0o555),
                vfs::permission::VfsMode::from_bits(0o444),
                vfs::permission::VfsMode::from_bits(0o555),
            ),
        });
    }
    Ok(())
}

fn plugin_development_permissions() -> vfs::permission::MountPermissions {
    use vfs::permission::{VfsAccess, VfsMode, VfsPathRule};

    let deny = |path: &str| VfsPathRule::Deny {
        path: path.to_string(),
        access: vec![VfsAccess::Read, VfsAccess::Write, VfsAccess::Execute],
    };
    vfs::permission::MountPermissions::unix(
        vfs::permission::VfsOwnership {
            uid: vfs::permission::VFS_ROOT_UID,
            gid: vfs::permission::VFS_PLUGIN_DEVELOPER_GID,
        },
        VfsMode::from_bits(0o775),
        VfsMode::from_bits(0o664),
        VfsMode::from_bits(0o775),
    )
    .with_rules(vec![
        deny(".git"),
        deny(".git/**"),
        deny("**/.git"),
        deny("**/.git/**"),
        deny(".autonomics-layout.toml"),
        VfsPathRule::Deny {
            path: "*/manifest.toml".into(),
            access: vec![VfsAccess::Write],
        },
        VfsPathRule::Deny {
            path: "Dockerfile".into(),
            access: vec![VfsAccess::Write],
        },
        VfsPathRule::Deny {
            path: "**/Dockerfile".into(),
            access: vec![VfsAccess::Write],
        },
    ])
}

/// Mount `<state_dir>/environments/dev` at `/environments/dev`.
///
/// Unlike the plugin mount this is unconditional: environment workspaces are
/// a fresh namespace, so no pre-existing registry file gates the mount.
fn ensure_environment_dev_mount(
    manifest: &mut vfs::VfsManifest,
    config: &RuntimeConfig,
) -> Result<()> {
    const BACKEND_ID: &str = "autonomics-environment-development";
    if manifest
        .backend
        .iter()
        .any(|backend| backend.id == BACKEND_ID)
    {
        return Err(Error::Other(
            "environment development backend is reserved for lifecycle-managed mounts".into(),
        ));
    }
    let root = config.state_dir.join("environments").join("dev");
    std::fs::create_dir_all(&root)
        .map_err(|error| Error::Other(format!("create `{}`: {error}", root.display())))?;
    manifest.backend.push(vfs::BackendDefinition {
        id: BACKEND_ID.into(),
        config: vfs::BackendConfig::local(root.to_string_lossy().into_owned()),
    });
    manifest.mount.push(vfs::MountDefinition {
        domain: None,
        path: plugin_rsi::ENVIRONMENT_DEVELOPMENT_VFS_ROOT.into(),
        backend: BACKEND_ID.into(),
        source: "/".into(),
        read_only: false,
        permissions: environment_development_permissions(),
    });
    Ok(())
}

/// Permissions for environment workspaces: the plugin deny rules minus the
/// Dockerfile/Containerfile write ban — authoring the Containerfile is the
/// entire point of this mount.
fn environment_development_permissions() -> vfs::permission::MountPermissions {
    use vfs::permission::{VfsAccess, VfsMode, VfsPathRule};

    let deny = |path: &str| VfsPathRule::Deny {
        path: path.to_string(),
        access: vec![VfsAccess::Read, VfsAccess::Write, VfsAccess::Execute],
    };
    vfs::permission::MountPermissions::unix(
        vfs::permission::VfsOwnership {
            uid: vfs::permission::VFS_ROOT_UID,
            gid: vfs::permission::VFS_PLUGIN_DEVELOPER_GID,
        },
        VfsMode::from_bits(0o775),
        VfsMode::from_bits(0o664),
        VfsMode::from_bits(0o775),
    )
    .with_rules(vec![
        deny(".git"),
        deny(".git/**"),
        deny("**/.git"),
        deny("**/.git/**"),
        VfsPathRule::Deny {
            path: "*/manifest.toml".into(),
            access: vec![VfsAccess::Write],
        },
    ])
}

/// Map an agent path to a stable, non-root Unix identity.
///
/// All agents initially receive the plugin-developer group; kind-specific
/// group membership can replace this deterministic mapping later.
pub(super) fn agent_vfs_principal(agent_path: &str) -> vfs::permission::VfsPrincipal {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in agent_path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let uid = 10_000_u32 + u32::try_from(hash % 55_536).unwrap_or_default();
    vfs::permission::VfsPrincipal::plugin_developer(uid)
}

struct VfsManifestState {
    manifest: VfsManifest,
    catalog_source: Option<String>,
}

/// Read `state_dir/vfs.toml`, preserve sections unknown to `VfsManifest`, and
/// materialize the default manifest on first launch.
///
/// The catalog section is tracked separately because `VfsManifest` models only
/// backends and mounts; rewriting the TOML without this source would discard
/// runtime catalog configuration.
fn load_or_create_vfs_manifest(config: &RuntimeConfig) -> Result<VfsManifestState> {
    let manifest_path = config.state_dir.join("vfs.toml");
    match std::fs::read_to_string(&manifest_path) {
        Ok(source) => {
            let catalog_source = extract_catalog_section(&source);
            let mut manifest = VfsManifest::from_toml(&source)
                .map_err(|e| Error::Other(format!("invalid {}: {e}", manifest_path.display())))?;
            let domains_changed = manifest.migrate_domains();
            if domains_changed {
                // Edit only domain fields in the source document: preserve
                // catalog and operator extensions unknown to VfsManifest.
                let mut document: toml::Value = toml::from_str(&source)
                    .map_err(|e| Error::Other(format!("parse VFS migration: {e}")))?;
                if let Some(mounts) = document
                    .get_mut("mount")
                    .and_then(toml::Value::as_array_mut)
                {
                    for mount in mounts {
                        let path = mount.get("path").and_then(toml::Value::as_str);
                        let domain = manifest
                            .mount
                            .iter()
                            .find(|m| Some(m.path.as_str()) == path)
                            .and_then(|m| m.domain.clone());
                        if let (Some(table), Some(domain)) = (mount.as_table_mut(), domain) {
                            table.insert("domain".into(), toml::Value::String(domain));
                        }
                    }
                }
                let migrated =
                    toml::to_string_pretty(&document).map_err(|e| Error::Other(e.to_string()))?;
                let backup = manifest_path.with_extension("toml.pre-domain.bak");
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                use std::io::Write;
                match options.open(&backup) {
                    Ok(mut file) => file
                        .write_all(source.as_bytes())
                        .and_then(|_| file.sync_all())
                        .map_err(|e| Error::Other(format!("backup VFS manifest: {e}")))?,
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(Error::Other(format!("backup VFS manifest: {e}"))),
                }
                let mut staged = tempfile::NamedTempFile::new_in(&config.state_dir)
                    .map_err(|e| Error::Other(e.to_string()))?;
                staged
                    .write_all(migrated.as_bytes())
                    .and_then(|_| staged.as_file().sync_all())
                    .map_err(|e| Error::Other(e.to_string()))?;
                staged
                    .persist(&manifest_path)
                    .map_err(|e| Error::Other(e.to_string()))?;
            }
            let literature_changed = ensure_literature_mount(&mut manifest, config);
            let browser_downloads_changed = ensure_browser_download_mount(&mut manifest, config)?;
            let wiki_attachments_changed = ensure_wiki_attachments_mount(&mut manifest, config)?;
            let changed =
                literature_changed || browser_downloads_changed || wiki_attachments_changed;
            if changed {
                write_vfs_manifest(&manifest_path, &manifest, catalog_source.as_deref())?;
            }
            Ok(VfsManifestState {
                manifest,
                catalog_source,
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let manifest = default_vfs_manifest(config);
            write_vfs_manifest(&manifest_path, &manifest, None)?;
            Ok(VfsManifestState {
                manifest,
                catalog_source: None,
            })
        }
        Err(e) => Err(Error::Other(format!(
            "read {}: {e}",
            manifest_path.display()
        ))),
    }
}

fn write_vfs_manifest(
    path: &std::path::Path,
    manifest: &VfsManifest,
    catalog_source: Option<&str>,
) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Err(Error::Other(format!(
            "VFS manifest path has no parent: {}",
            path.display()
        )));
    };
    std::fs::create_dir_all(parent).map_err(|e| {
        Error::Other(format!(
            "create VFS manifest directory {}: {e}",
            parent.display()
        ))
    })?;

    let mut document = match std::fs::read_to_string(path) {
        Ok(source) => {
            toml::from_str::<toml::Value>(&source).map_err(|e| Error::Other(e.to_string()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            toml::Value::Table(Default::default())
        }
        Err(e) => return Err(Error::Other(e.to_string())),
    };
    let typed = toml::Value::try_from(manifest).map_err(|e| Error::Other(e.to_string()))?;
    for (section, identity) in [("backend", "id"), ("mount", "path")] {
        let old = document
            .get(section)
            .and_then(toml::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut entries = typed
            .get(section)
            .and_then(toml::Value::as_array)
            .cloned()
            .unwrap_or_default();
        for entry in &mut entries {
            if let Some(previous) = old
                .iter()
                .find(|old| old.get(identity) == entry.get(identity))
                && let (Some(previous), Some(current)) = (previous.as_table(), entry.as_table_mut())
            {
                for (key, value) in previous {
                    current.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
        }
        document
            .as_table_mut()
            .ok_or_else(|| Error::Other("VFS manifest must be a table".into()))?
            .insert(section.into(), toml::Value::Array(entries));
    }
    if document.get("catalog").is_none()
        && let Some(catalog) = catalog_source
    {
        let catalog: toml::Value =
            toml::from_str(catalog).map_err(|e| Error::Other(e.to_string()))?;
        if let Some(value) = catalog.get("catalog") {
            document
                .as_table_mut()
                .unwrap()
                .insert("catalog".into(), value.clone());
        }
    }
    let source = toml::to_string_pretty(&document).map_err(|e| Error::Other(e.to_string()))?;
    use std::io::Write;
    let mut staged =
        tempfile::NamedTempFile::new_in(parent).map_err(|e| Error::Other(e.to_string()))?;
    staged
        .write_all(source.as_bytes())
        .and_then(|_| staged.as_file().sync_all())
        .map_err(|e| Error::Other(format!("write {}: {e}", path.display())))?;
    staged
        .persist(path)
        .map_err(|e| Error::Other(format!("replace {}: {e}", path.display())))?;
    Ok(())
}

fn extract_catalog_section(source: &str) -> Option<String> {
    let mut section = String::new();
    let mut active = false;
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            active = trimmed == "[catalog]";
        }
        if active {
            section.push_str(line);
            section.push('\n');
        }
    }
    (!section.trim().is_empty()).then_some(section)
}

fn default_vfs_manifest(config: &RuntimeConfig) -> VfsManifest {
    let mut backend = vec![BackendDefinition {
        id: "default".into(),
        config: vfs::BackendConfig::local("/"),
    }];
    let mut mount = vec![MountDefinition {
        domain: Some("local".into()),
        path: "/".into(),
        backend: "default".into(),
        source: config.data_dir.to_string_lossy().to_string(),
        read_only: false,
        permissions: Default::default(),
    }];
    let literature_root = config.state_dir.join("literature");
    backend.push(BackendDefinition {
        id: "literature".into(),
        config: vfs::BackendConfig::local(literature_root.to_string_lossy().to_string()),
    });
    mount.push(MountDefinition {
        domain: None,
        path: "/literature".into(),
        backend: "literature".into(),
        source: "/".into(),
        read_only: false,
        permissions: Default::default(),
    });
    let browser_download_root = browser_download_root(config);
    backend.push(BackendDefinition {
        id: "browser-downloads".into(),
        config: vfs::BackendConfig::local(browser_download_root.to_string_lossy().to_string()),
    });
    mount.push(MountDefinition {
        domain: None,
        path: BROWSER_DOWNLOAD_VFS_PATH.into(),
        backend: "browser-downloads".into(),
        source: "/".into(),
        read_only: false,
        permissions: Default::default(),
    });
    backend.push(BackendDefinition {
        id: "wiki-attachments".into(),
        config: vfs::BackendConfig::local(
            wiki_attachments_root(config).to_string_lossy().to_string(),
        ),
    });
    mount.push(MountDefinition {
        domain: None,
        path: WIKI_ATTACHMENTS_VFS_PATH.into(),
        backend: "wiki-attachments".into(),
        source: "/".into(),
        read_only: false,
        permissions: Default::default(),
    });

    if let (Ok(bucket), Ok(ak), Ok(sk)) = (
        std::env::var("OSS_BUCKET"),
        std::env::var("OSS_ACCESS_KEY_ID"),
        std::env::var("OSS_SECRET_ACCESS_KEY"),
    ) {
        let endpoint =
            std::env::var("OSS_ENDPOINT").unwrap_or_else(|_| "oss-cn-beijing.aliyuncs.com".into());
        backend.push(BackendDefinition {
            id: "oss-prod".into(),
            config: vfs::BackendConfig::oss(bucket, endpoint, ak, sk),
        });
        mount.push(MountDefinition {
            domain: Some("oss".into()),
            path: "/data/oss".into(),
            backend: "oss-prod".into(),
            source: "/".into(),
            read_only: true,
            permissions: Default::default(),
        });
    }

    VfsManifest { backend, mount }
}

/// Keep the literature namespace mounted even for manifests created before
/// bibliography storage used the VFS.
fn ensure_literature_mount(manifest: &mut VfsManifest, config: &RuntimeConfig) -> bool {
    if manifest
        .mount
        .iter()
        .any(|mount| mount.path == "/literature")
    {
        return false;
    }

    let mut backend_id = "literature".to_owned();
    while manifest
        .backend
        .iter()
        .any(|backend| backend.id == backend_id)
    {
        backend_id.push('_');
    }
    manifest.backend.push(BackendDefinition {
        id: backend_id.clone(),
        config: vfs::BackendConfig::local(
            config
                .state_dir
                .join("literature")
                .to_string_lossy()
                .to_string(),
        ),
    });
    manifest.mount.push(MountDefinition {
        domain: None,
        path: "/literature".into(),
        backend: backend_id,
        source: "/".into(),
        read_only: false,
        permissions: Default::default(),
    });
    true
}

/// Keep a local directory mounted at `vfs_path` even for manifests created
/// before the capability existed. A custom configured root retargets the
/// existing mount (shared backends get a fresh one instead of being mutated).
///
/// Returns whether the manifest changed and needs persisting.
fn ensure_local_dir_mount(
    manifest: &mut VfsManifest,
    vfs_path: &str,
    backend_prefix: &str,
    root: &std::path::Path,
) -> Result<bool> {
    let root_text = root.to_string_lossy().to_string();
    let Some(mount_index) = manifest
        .mount
        .iter()
        .position(|mount| mount.path == vfs_path)
    else {
        let backend_id = unique_backend_id(manifest, backend_prefix);
        manifest.backend.push(BackendDefinition {
            id: backend_id.clone(),
            config: vfs::BackendConfig::local(root_text),
        });
        manifest.mount.push(MountDefinition {
            domain: None,
            path: vfs_path.into(),
            backend: backend_id,
            source: "/".into(),
            read_only: false,
            permissions: Default::default(),
        });
        return Ok(true);
    };

    let existing_backend_id = manifest.mount[mount_index].backend.clone();
    let Some(backend_index) = manifest
        .backend
        .iter()
        .position(|backend| backend.id == existing_backend_id)
    else {
        return Err(Error::Other(format!(
            "the `{vfs_path}` VFS mount has no backend"
        )));
    };
    let backend_root = match &manifest.backend[backend_index].config {
        vfs::BackendConfig::Local { root } => Some(root.clone()),
        _ => None,
    };
    if backend_root.as_deref() == Some(root_text.as_str())
        && manifest.mount[mount_index].source == "/"
    {
        return Ok(false);
    }

    let shared_backend = manifest
        .mount
        .iter()
        .enumerate()
        .any(|(index, mount)| index != mount_index && mount.backend == existing_backend_id);
    if shared_backend {
        let backend_id = unique_backend_id(manifest, backend_prefix);
        manifest.backend.push(BackendDefinition {
            id: backend_id.clone(),
            config: vfs::BackendConfig::local(root_text),
        });
        manifest.mount[mount_index].backend = backend_id;
    } else {
        manifest.backend[backend_index].config = vfs::BackendConfig::local(root_text);
    }
    manifest.mount[mount_index].source = "/".into();
    manifest.mount[mount_index].read_only = false;
    Ok(true)
}

/// Fresh backend id for `prefix`, suffixed with `_` on collision.
fn unique_backend_id(manifest: &VfsManifest, prefix: &str) -> String {
    let mut backend_id = prefix.to_owned();
    while manifest
        .backend
        .iter()
        .any(|backend| backend.id == backend_id)
    {
        backend_id.push('_');
    }
    backend_id
}

/// Keep browser downloads mounted even for manifests created before this
/// capability existed. A custom configured root retargets the existing mount.
fn ensure_browser_download_mount(
    manifest: &mut VfsManifest,
    config: &RuntimeConfig,
) -> Result<bool> {
    ensure_local_dir_mount(
        manifest,
        BROWSER_DOWNLOAD_VFS_PATH,
        "browser-downloads",
        &browser_download_root(config),
    )
}

/// Wiki attachment blobs live under their own VFS mount instead of the root
/// workspace, so cleaning or rebuilding the root mount never touches them.
/// Root: `state_dir/wiki-attachments` (see `open_wiki` for the blob writer).
fn ensure_wiki_attachments_mount(
    manifest: &mut VfsManifest,
    config: &RuntimeConfig,
) -> Result<bool> {
    ensure_local_dir_mount(
        manifest,
        WIKI_ATTACHMENTS_VFS_PATH,
        "wiki-attachments",
        &wiki_attachments_root(config),
    )
}

/// Ensure the container workspace tree is reachable through the VFS at its
/// own host-absolute path (identity mount), so the work-dir file refs
/// produced by container nodes resolve for every consumer — DataFusion
/// readers, artifact hashing, and RO-Crate export included.
///
/// Ephemeral by design: never written back to `vfs.toml`, so a moved
/// workspace root self-heals on the next start instead of leaving a stale
/// persisted entry. An existing mount at the same path keeps its backend
/// and only has its `source` retargeted when the root moved.
fn ensure_workspace_mount(manifest: &mut VfsManifest, workspace_root: &std::path::Path) -> bool {
    let root = workspace_root.to_string_lossy().to_string();
    if let Some(existing) = manifest.mount.iter_mut().find(|mount| mount.path == root) {
        if existing.source == root {
            return false;
        }
        existing.source = root.clone();
        return true;
    }

    // Reuse a local backend rooted at `/` when one exists (the default
    // manifest's `default` backend is exactly that); otherwise append one.
    let backend_id = manifest
        .backend
        .iter()
        .find(
            |backend| matches!(&backend.config, vfs::BackendConfig::Local { root } if root == "/"),
        )
        .map(|backend| backend.id.clone())
        .unwrap_or_else(|| {
            let mut id = "autonomics-workspace".to_owned();
            while manifest.backend.iter().any(|backend| backend.id == id) {
                id.push('_');
            }
            manifest.backend.push(BackendDefinition {
                id: id.clone(),
                config: vfs::BackendConfig::local("/"),
            });
            id
        });
    manifest.mount.push(MountDefinition {
        domain: None,
        path: root.clone(),
        backend: backend_id,
        source: root,
        read_only: true,
        permissions: Default::default(),
    });
    true
}

#[cfg(test)]
mod vfs_tests {
    use super::*;

    #[test]
    fn legacy_domains_migrate_once_with_backup_and_operator_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = RuntimeConfig::default();
        config.state_dir = dir.path().to_path_buf();
        config.data_dir = dir.path().join("data");
        let path = dir.path().join("vfs.toml");
        let source = "[operator]\nlabel='preserved'\n[[backend]]\nid='shared'\ntype='ssh'\nalias='fixture'\nroot='/share'\ncustom='backend-extension'\n[[mount]]\npath='/cluster'\nbackend='shared'\nsource='/'\nread_only=true\ncustom='mount-extension'\n[catalog]\nrepository='owner/index'\n";
        std::fs::write(&path, source).unwrap();
        let first = load_or_create_vfs_manifest(&config).unwrap();
        assert_eq!(
            first
                .manifest
                .mount
                .iter()
                .find(|m| m.path == "/cluster")
                .unwrap()
                .domain
                .as_deref(),
            Some("shared")
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("backend-extension")
                && text.contains("mount-extension")
                && text.contains("preserved")
        );
        assert_eq!(
            std::fs::read_to_string(path.with_extension("toml.pre-domain.bak")).unwrap(),
            source
        );
        load_or_create_vfs_manifest(&config).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn build_vfs_materializes_default_manifest_on_first_launch() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = RuntimeConfig::default();
        config.data_dir = dir.path().join("data");
        config.state_dir = dir.path().join("state");
        let manifest_path = config.state_dir.join("vfs.toml");

        build_vfs(&config).unwrap();
        let source = std::fs::read_to_string(&manifest_path).unwrap();
        let manifest = VfsManifest::from_toml(&source).unwrap();
        assert_eq!(
            manifest.mount[0].source,
            config.data_dir.to_string_lossy().to_string()
        );
    }

    #[tokio::test]
    async fn browser_downloads_mount_into_vfs() {
        let state = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let mut config = RuntimeConfig::default();
        config.data_dir = data.path().to_path_buf();
        config.state_dir = state.path().to_path_buf();
        let root = browser_download_root(&config);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("result.csv"), b"a,b\n1,2\n").unwrap();
        let legacy_manifest =
            VfsManifest::local_root(config.data_dir.to_string_lossy().to_string());
        std::fs::create_dir_all(&config.state_dir).unwrap();
        std::fs::write(
            config.state_dir.join("vfs.toml"),
            toml::to_string_pretty(&legacy_manifest).unwrap(),
        )
        .unwrap();

        let store = Arc::new(build_vfs(&config).unwrap());
        let storage = vfs::OpendalFileStorage::with_mounts(&config.data_dir, store);
        let path = "/browser-downloads/result.csv";
        let bytes = storage
            .resolve(path)
            .read(&storage.resolve_path(path))
            .await
            .unwrap();

        assert_eq!(bytes.to_vec(), b"a,b\n1,2\n");
        let manifest = VfsManifest::from_toml(
            &std::fs::read_to_string(config.state_dir.join("vfs.toml")).unwrap(),
        )
        .unwrap();
        assert!(
            manifest
                .mount
                .iter()
                .any(|mount| mount.path == "/browser-downloads")
        );
    }

    #[tokio::test]
    async fn build_vfs_mounts_catalog_entries_and_preserves_catalog_config() {
        let state = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let input = scratch.path().join("input");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("data.txt"), b"catalog-data").unwrap();
        let package = scratch.path().join("package");
        data_catalog::build_package(
            input,
            &package,
            data_catalog::package::BuildOptions {
                repo: Some("owner/catalog-panel".into()),
                version: Some("v1".into()),
                kind: Some("table".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let mut config = RuntimeConfig::default();
        config.data_dir = data.path().to_path_buf();
        config.state_dir = state.path().to_path_buf();
        std::fs::write(
            config.state_dir.join("vfs.toml"),
            r#"
[catalog]
repository = "owner/catalog-index"
"#,
        )
        .unwrap();
        struct PanelCacheRootEnv;
        impl Drop for PanelCacheRootEnv {
            fn drop(&mut self) {
                // SAFETY: no other host test reads this variable concurrently.
                unsafe { std::env::remove_var("AUTONOMICS_PANEL_CACHE_ROOT") };
            }
        }
        let panel_root = tempfile::tempdir().unwrap();
        let _panel_env = PanelCacheRootEnv;
        // SAFETY: the host resolves this variable in build_vfs_with_catalog below.
        unsafe { std::env::set_var("AUTONOMICS_PANEL_CACHE_ROOT", panel_root.path()) };
        let manifest: data_catalog::DatasetManifest =
            serde_json::from_slice(&std::fs::read(package.join("manifest.json")).unwrap()).unwrap();
        let entry = data_catalog::CatalogEntry {
            repo: data_catalog::HfRepoId::new("owner/catalog-panel").unwrap(),
            version: manifest.version.clone(),
            kind: manifest.kind.clone(),
            digest: manifest.digest.clone().expect("test package has digest"),
            current: true,
            created_unix_seconds: 1,
        };
        let cache_index = data_catalog::CatalogIndex {
            repositories: vec![entry.repo.clone()],
            entries: vec![entry.clone()],
            ..data_catalog::CatalogIndex::default()
        };
        let entry_root = panel_root
            .path()
            .join(format!("{}@{}", entry.repo, entry.digest));
        std::fs::create_dir_all(&entry_root).unwrap();
        std::fs::copy(
            package.join("payload").join("data.txt"),
            entry_root.join("data.txt"),
        )
        .unwrap();
        std::fs::write(
            entry_root.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(
            entry_root.join(".autonomics-panel-complete"),
            format!("digest={}\n", entry.digest),
        )
        .unwrap();
        std::fs::write(
            panel_root.path().join("index.json"),
            serde_json::to_vec(&cache_index).unwrap(),
        )
        .unwrap();

        let (store, bundles, catalog_service) = build_vfs_with_catalog(&config).await.unwrap();
        let catalog_state = catalog_service.expect("enabled catalog service");
        assert_eq!(catalog_state.local.index().unwrap().entries.len(), 1);
        let storage = Arc::new(vfs::OpendalFileStorage::with_mounts(
            &config.data_dir,
            Arc::new(store),
        ));
        let bytes = storage
            .resolve("/bundles/owner/catalog-panel/data.txt")
            .read(&storage.resolve_path("/bundles/owner/catalog-panel/data.txt"))
            .await
            .unwrap();
        assert_eq!(bytes.to_vec(), b"catalog-data");
        assert_eq!(
            bundles
                .get("owner/catalog-panel")
                .map(|bundle| bundle.vpath.as_str())
                .unwrap(),
            "/bundles/owner/catalog-panel"
        );
        let persisted = std::fs::read_to_string(config.state_dir.join("vfs.toml")).unwrap();
        assert!(persisted.contains("[catalog]"));
    }

    #[test]
    fn generated_manifest_is_writable_and_parses_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("vfs.toml");
        let manifest =
            VfsManifest::local_root(dir.path().join("data").to_string_lossy().to_string());

        write_vfs_manifest(&path, &manifest, None).unwrap();

        let source = std::fs::read_to_string(&path).unwrap();
        let parsed = VfsManifest::from_toml(&source).unwrap();
        assert_eq!(parsed, manifest);
    }
}

#[cfg(test)]
mod literature_mount_tests {
    use super::*;
    use vfs::{BackendDefinition, MountDefinition, VfsManifest};

    fn base_manifest() -> VfsManifest {
        VfsManifest {
            backend: vec![BackendDefinition {
                id: "default".into(),
                config: vfs::BackendConfig::local("/"),
            }],
            mount: vec![MountDefinition {
                domain: None,
                path: "/".into(),
                backend: "default".into(),
                source: "/".into(),
                read_only: false,
                permissions: Default::default(),
            }],
        }
    }

    #[test]
    fn browser_download_mount_adds_or_retargets_dedicated_backend() {
        let state = tempfile::tempdir().unwrap();
        let custom = tempfile::tempdir().unwrap();
        let mut config = RuntimeConfig::default();
        config.state_dir = state.path().to_path_buf();
        config.browser_download_dir = Some(custom.path().to_path_buf());

        let mut manifest = base_manifest();
        assert!(ensure_browser_download_mount(&mut manifest, &config).unwrap());
        let mount = manifest
            .mount
            .iter()
            .find(|mount| mount.path == BROWSER_DOWNLOAD_VFS_PATH)
            .unwrap();
        assert_eq!(mount.source, "/");
        assert!(!mount.read_only);
        assert_eq!(mount.backend, "browser-downloads");
        let backend = manifest
            .backend
            .iter()
            .find(|backend| backend.id == "browser-downloads")
            .unwrap();
        assert_eq!(
            backend.config,
            vfs::BackendConfig::local(custom.path().to_string_lossy().to_string())
        );

        assert!(!ensure_browser_download_mount(&mut manifest, &config).unwrap());
        let moved = tempfile::tempdir().unwrap();
        config.browser_download_dir = Some(moved.path().to_path_buf());
        assert!(ensure_browser_download_mount(&mut manifest, &config).unwrap());
        assert_eq!(
            manifest
                .backend
                .iter()
                .find(|backend| backend.id == "browser-downloads")
                .unwrap()
                .config,
            vfs::BackendConfig::local(moved.path().to_string_lossy().to_string())
        );
    }

    #[test]
    fn wiki_attachments_mount_pins_state_dir_root() {
        let state = tempfile::tempdir().unwrap();
        let mut config = RuntimeConfig::default();
        config.state_dir = state.path().to_path_buf();

        let mut manifest = base_manifest();
        assert!(ensure_wiki_attachments_mount(&mut manifest, &config).unwrap());
        let expected_root = state.path().join("wiki-attachments");
        let mount = manifest
            .mount
            .iter()
            .find(|mount| mount.path == WIKI_ATTACHMENTS_VFS_PATH)
            .unwrap();
        assert_eq!(mount.source, "/");
        assert!(!mount.read_only);
        let backend = manifest
            .backend
            .iter()
            .find(|backend| backend.id == "wiki-attachments")
            .unwrap();
        assert_eq!(
            backend.config,
            vfs::BackendConfig::local(expected_root.to_string_lossy().to_string())
        );
        // Idempotent once pinned.
        assert!(!ensure_wiki_attachments_mount(&mut manifest, &config).unwrap());
    }

    #[tokio::test]
    async fn wiki_blob_prefix_resolves_onto_dedicated_mount() {
        // 端到端:VfsBlobStore 的 `wiki-attachments/<key>` 前缀经挂载表
        // 落到 state_dir 独立目录,不进根挂载工作区。
        let state = tempfile::tempdir().unwrap();
        let mut config = RuntimeConfig::default();
        config.state_dir = state.path().to_path_buf();
        config.data_dir = state.path().join("data");

        let mut manifest = default_vfs_manifest(&config);
        ensure_wiki_attachments_mount(&mut manifest, &config).unwrap();
        let mounts = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = vfs::OpendalFileStorage::with_mounts(&config.data_dir, mounts);
        let blobs = wiki::storage::vfs_blob::VfsBlobStore::new(storage, "wiki-attachments");
        use wiki::storage::blob::BlobStore as _;

        blobs
            .put("attachments/e1/a1.png", b"png".to_vec())
            .await
            .unwrap();
        assert!(
            config
                .data_dir
                .join("wiki-attachments/attachments/e1/a1.png")
                .exists()
                == false
        );
        let expected = state.path().join("wiki-attachments/attachments/e1/a1.png");
        assert!(expected.exists());
        assert_eq!(
            blobs.get("attachments/e1/a1.png").await.unwrap(),
            b"png".to_vec()
        );
        blobs.delete("attachments/e1/a1.png").await.unwrap();
        assert!(!expected.exists());
    }

    #[test]
    fn ensure_workspace_mount_adds_identity_mount_reusing_root_backend() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().to_string_lossy().to_string();
        let mut manifest = base_manifest();
        assert!(ensure_workspace_mount(&mut manifest, workspace.path()));
        let mount = manifest
            .mount
            .iter()
            .find(|mount| mount.path == root)
            .expect("workspace identity mount inserted");
        // Reuses the existing `/`-rooted local backend, no new backend added.
        assert_eq!(mount.backend, "default");
        assert_eq!(mount.source, root);
        assert!(mount.read_only);
        assert_eq!(manifest.backend.len(), 1);
        // Idempotent on the second pass.
        assert!(!ensure_workspace_mount(&mut manifest, workspace.path()));
        // A moved workspace root retargets the existing mount in place.
        let moved = tempfile::tempdir().unwrap();
        let moved_root = moved.path().to_string_lossy().to_string();
        assert!(ensure_workspace_mount(&mut manifest, moved.path()));
        let mount = manifest
            .mount
            .iter()
            .find(|mount| mount.path == moved_root)
            .expect("retargeted mount");
        assert_eq!(mount.source, moved_root);
    }

    #[tokio::test]
    async fn workspace_mount_resolves_host_paths_and_rejects_writes() {
        let workspace = tempfile::tempdir().unwrap();
        let work_dir = workspace.path().join("ab/cdef");
        std::fs::create_dir_all(&work_dir).unwrap();
        std::fs::write(work_dir.join("result.csv"), "a,b\n1,2\n").unwrap();

        let mut manifest = base_manifest();
        ensure_workspace_mount(&mut manifest, workspace.path());
        let data_dir = tempfile::tempdir().unwrap();
        let mounts = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = vfs::OpendalFileStorage::with_mounts(data_dir.path(), mounts);

        // The bare host path resolves to the real file — both via the host
        // accessor and through a full read (the DataFusion reader path).
        let host_file = work_dir.join("result.csv");
        assert_eq!(
            storage.local_host_path(&host_file.to_string_lossy()),
            Some(host_file.clone())
        );
        let key = storage.resolve_path(&host_file.to_string_lossy());
        let operator = storage.resolve(&host_file.to_string_lossy());
        let bytes = operator.read(&key).await.unwrap();
        assert_eq!(bytes.to_vec(), b"a,b\n1,2\n");
        // The identity mount is read-only: writes are denied.
        assert!(
            storage
                .check_writable(&host_file.to_string_lossy())
                .is_err()
        );
    }

    #[test]
    fn ensure_literature_mount_adds_default_backend() {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::config::RuntimeConfigBuilder::default()
            .data_dir(directory.path().join("data"))
            .state_dir(directory.path().to_path_buf())
            .bib_db_path(directory.path().join("bib.db"))
            .agent_db(directory.path().join("agent.db"))
            .app_db_path(directory.path().join("app.db"))
            .dag_history_db(directory.path().join("dag.db"))
            .build();
        let mut manifest = base_manifest();
        assert!(ensure_literature_mount(&mut manifest, &config));
        let literature_mount = manifest
            .mount
            .iter()
            .find(|mount| mount.path == "/literature")
            .expect("literature mount inserted");
        let backend = manifest
            .backend
            .iter()
            .find(|backend| backend.id == literature_mount.backend)
            .expect("literature backend inserted");
        match &backend.config {
            vfs::BackendConfig::Local { root } => {
                assert_eq!(
                    root,
                    &config
                        .state_dir
                        .join("literature")
                        .to_string_lossy()
                        .to_string()
                );
            }
            other => panic!("expected local backend, got {other:?}"),
        }
        assert!(!literature_mount.read_only);
    }

    #[test]
    fn ensure_literature_mount_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::config::RuntimeConfigBuilder::default()
            .data_dir(directory.path().join("data"))
            .state_dir(directory.path().to_path_buf())
            .bib_db_path(directory.path().join("bib.db"))
            .agent_db(directory.path().join("agent.db"))
            .app_db_path(directory.path().join("app.db"))
            .dag_history_db(directory.path().join("dag.db"))
            .build();
        let mut manifest = base_manifest();
        assert!(ensure_literature_mount(&mut manifest, &config));
        let mount_count = manifest.mount.len();
        let backend_count = manifest.backend.len();
        assert!(!ensure_literature_mount(&mut manifest, &config));
        assert_eq!(manifest.mount.len(), mount_count);
        assert_eq!(manifest.backend.len(), backend_count);
    }
}

#[cfg(test)]
mod plugin_vfs_tests {
    use super::*;

    #[tokio::test]
    async fn plugin_mounts_apply_lifecycle_permissions() {
        let state = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let runtime_root = state.path().join("plugin-runtime");
        let plugin_root = runtime_root.join("demo-plugin");
        let development_root = state.path().join("plugins");
        let development_plugin = development_root.join("demo-plugin");
        std::fs::create_dir_all(&plugin_root).unwrap();
        std::fs::create_dir_all(development_plugin.join(".git")).unwrap();
        std::fs::write(plugin_root.join("manifest.toml"), "# demo\n").unwrap();
        std::fs::write(development_plugin.join("manifest.toml"), "# dev\n").unwrap();
        std::fs::write(
            development_plugin.join(".git").join("HEAD"),
            "ref: refs/heads/main\n",
        )
        .unwrap();
        std::fs::write(
            state.path().join("plugins.toml"),
            format!(
                "[[plugin]]\nname = \"demo-plugin\"\npath = \"{}\"\nlocal_commit = \"{}\"\nlocal_digest = \"{}\"\n",
                plugin_root.display(),
                "0123456789abcdef0123456789abcdef01234567",
                "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            ),
        )
        .unwrap();

        let mut config = RuntimeConfig::default();
        config.state_dir = state.path().to_path_buf();
        config.data_dir = data.path().to_path_buf();
        let plugin_layout = plugin_rsi::PluginStateLayout::open(&config.state_dir);
        let mut manifest = VfsManifest::local_root(data.path().to_string_lossy().into_owned());
        ensure_plugin_mounts(&mut manifest, &config, &plugin_layout).unwrap();
        let mounts = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = vfs::OpendalFileStorage::with_mounts(data.path(), mounts);
        let developer =
            storage.with_principal(vfs::permission::VfsPrincipal::plugin_developer(10_000));
        let observer = storage.with_principal(vfs::permission::VfsPrincipal::new(20_000, 20_000));

        let contents = storage
            .read_range("/plugins/active/demo-plugin/manifest.toml", 0..7)
            .await
            .unwrap();
        assert_eq!(contents.to_vec(), b"# demo\n");
        assert!(
            storage
                .write_bytes(
                    "/plugins/active/demo-plugin/manifest.toml",
                    b"changed".to_vec()
                )
                .await
                .is_err()
        );
        developer
            .write_bytes("/plugins/dev/demo-plugin/README.md", b"hello".to_vec())
            .await
            .unwrap();
        let git_access = developer.check_readable("/plugins/dev/demo-plugin/.git/HEAD");
        assert!(git_access.is_err(), "git_access={git_access:?}");
        assert!(
            developer
                .write_bytes(
                    "/plugins/dev/demo-plugin/manifest.toml",
                    b"changed".to_vec()
                )
                .await
                .is_err()
        );
        assert!(
            observer
                .read_range("/plugins/dev/demo-plugin/README.md", 0..5)
                .await
                .is_ok()
        );
        assert!(
            observer
                .write_bytes("/plugins/dev/demo-plugin/README.md", b"changed".to_vec())
                .await
                .is_err()
        );
    }
}

#[cfg(test)]
mod executor_workspace_mount_tests {
    use super::*;

    /// Both private storage layouts must override the coordinator's catch-all
    /// root, including when the overlay directory is a symlink to another disk.
    #[tokio::test]
    async fn executor_workspace_mount_beats_catch_all_root_mount() {
        let home = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let catch_all = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let overlay = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(overlay.path(), state.path().join("containers-overlay"))
            .unwrap();
        #[cfg(unix)]
        let moved_work = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(moved_work.path(), overlay.path().join("workspace")).unwrap();
        let mut manifest = VfsManifest {
            backend: vec![vfs::BackendDefinition {
                id: "default".into(),
                config: vfs::BackendConfig::local("/"),
            }],
            mount: vec![vfs::MountDefinition {
                domain: None,
                path: "/".into(),
                backend: "default".into(),
                source: catch_all.path().to_string_lossy().into_owned(),
                read_only: false,
                permissions: Default::default(),
            }],
        };
        let config = crate::RuntimeConfig {
            state_dir: state.path().to_path_buf(),
            ..Default::default()
        };

        // Reproduce the original failure before adding executor mounts.
        let unmapped = vfs::OpendalFileStorage::with_mounts(
            home.path(),
            Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap()),
        );
        let mut outputs = Vec::new();
        // Cover the new root and all historical roots, including overlay
        // storage relocated through a symlink to another disk.
        for relative in [
            "work",
            "state/work",
            "containers/workspace",
            "containers-overlay/workspace",
        ] {
            let candidate = state.path().join(relative);
            std::fs::create_dir_all(&candidate).unwrap();
            let root = container_runtime::canonicalize_workspace_root(candidate.clone());
            let output = root.join("02/f957/out.zip");
            std::fs::create_dir_all(output.parent().unwrap()).unwrap();
            std::fs::write(&output, b"payload").unwrap();
            assert!(
                unmapped
                    .content_length(&output.to_string_lossy())
                    .await
                    .is_err()
            );
            outputs.push(output);
            outputs.push(candidate.join("02/f957/out.zip"));
            let physical_parent = candidate.parent().unwrap().canonicalize().unwrap();
            outputs.push(
                physical_parent
                    .join(candidate.file_name().unwrap())
                    .join("02/f957/out.zip"),
            );
        }

        ensure_executor_workspace_mounts(&mut manifest, &config);
        let count = manifest.mount.len();
        ensure_executor_workspace_mounts(&mut manifest, &config);
        assert_eq!(
            manifest.mount.len(),
            count,
            "mount assembly must be idempotent"
        );
        let mounts = Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = vfs::OpendalFileStorage::with_mounts(home.path(), mounts.clone());
        use datafusion::object_store::ObjectStoreExt;
        use futures::TryStreamExt;
        for output in outputs {
            let path = output.to_string_lossy();
            assert_eq!(
                storage.local_host_path(&path).as_deref(),
                Some(output.as_path())
            );
            assert_eq!(storage.content_length(&path).await.unwrap(), 7);
            let chunks: Vec<_> = storage
                .read_stream(&path, 0..7)
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            assert_eq!(chunks.concat(), b"payload");
            assert!(storage.check_writable(&path).is_err());

            // The DataFusion/scheduler object-store adapter must see it too.
            let key = datafusion::object_store::path::Path::from(path.trim_start_matches('/'));
            assert_eq!(mounts.head(&key).await.unwrap().size, 7);
            assert_eq!(
                mounts
                    .get(&key)
                    .await
                    .unwrap()
                    .bytes()
                    .await
                    .unwrap()
                    .as_ref(),
                b"payload"
            );
        }
    }

    /// Operator diagnostic: reuse an existing ZIP without rerunning its container.
    /// AUTONOMICS_PUBLICATION_PROBE_ZIP is a host path returned by the executor;
    /// AUTONOMICS_PUBLICATION_PROBE_STATE selects the daemon's state directory.
    #[tokio::test]
    #[ignore = "requires an existing container ZIP and operator VFS manifest"]
    async fn existing_container_zip_reaches_archive_extract() {
        use datafusion::prelude::SessionContext;
        use sha2::{Digest, Sha256};

        let source = std::env::var("AUTONOMICS_PUBLICATION_PROBE_ZIP").unwrap();
        let state = std::env::var("AUTONOMICS_PUBLICATION_PROBE_STATE").unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let config = crate::RuntimeConfig {
            state_dir: state.into(),
            ..Default::default()
        };
        let text = std::fs::read_to_string(config.state_dir.join("vfs.toml")).unwrap();
        let mut manifest = VfsManifest::from_toml(&text).unwrap();
        ensure_executor_workspace_mounts(&mut manifest, &config);
        manifest.backend.push(BackendDefinition {
            id: "publication-probe".into(),
            config: BackendConfig::local(scratch.path().to_string_lossy().into_owned()),
        });
        manifest.mount.push(MountDefinition {
            domain: None,
            path: "/publication-probe".into(),
            backend: "publication-probe".into(),
            source: "/".into(),
            read_only: false,
            permissions: Default::default(),
        });
        let mounts = Arc::new(MountedObjectStore::from_manifest(&manifest).unwrap());
        let storage = Arc::new(vfs::OpendalFileStorage::with_mounts(
            scratch.path(),
            mounts.clone(),
        ));
        let original = std::fs::read(&source).unwrap();
        assert_eq!(
            storage.local_host_path(&source),
            Some(std::path::PathBuf::from(&source))
        );
        assert_eq!(
            storage.content_length(&source).await.unwrap(),
            original.len() as u64
        );
        let bytes = storage
            .read_range(&source, 0..original.len() as u64)
            .await
            .unwrap();
        assert_eq!(bytes.to_vec(), original);
        let session = SessionContext::new();
        session.runtime_env().register_object_store(
            datafusion::execution::object_store::ObjectStoreUrl::parse("local://")
                .unwrap()
                .as_ref(),
            mounts,
        );
        let registry =
            data_engine::default_registry::build_default_registry_with_container_execution(
                session.runtime_env(),
                Some(storage.clone()),
                Arc::new(dag_core::BundleRegistry::new()),
                Arc::new(container_runtime::ContainerExecutionInfra::from_env()),
                Some(scratch.path().join("no-plugins")),
            );
        let mut node = registry
            .build_node(
                "archive_extract",
                serde_json::json!({
                    "src": format!("local://{source}"),
                    "destination_prefix": "/publication-probe/extracted"
                }),
            )
            .unwrap();
        let outputs = node
            .execute(
                registry.ctx(),
                &[],
                &dag_core::dag::node_event::NodeReporter::noop(),
            )
            .await
            .unwrap();
        let files = outputs.get(&0).unwrap().as_file_set().unwrap();
        assert!(!files.is_empty());
        for file in files {
            let path = vfs::uri::storage_path(&file.path).unwrap();
            let size = storage.content_length(path).await.unwrap();
            assert!(size > 0);
            assert_eq!(
                storage.read_range(path, 0..size).await.unwrap().len() as u64,
                size
            );
            println!(
                "archive_extract readable output: {} ({} bytes)",
                file.path, size
            );
        }
        println!(
            "original ZIP: {} bytes, sha256:{:x}",
            original.len(),
            Sha256::digest(&original)
        );
    }
}
