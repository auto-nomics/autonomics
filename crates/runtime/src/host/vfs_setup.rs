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
    ensure_workspace_mount(&mut manifest, &PodmanConfig::from_env().workspace_root);
    ensure_environment_dev_mount(&mut manifest, config)?;
    MountedObjectStore::from_manifest(&manifest).map_err(|e| Error::Other(e.to_string()))
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
    ensure_workspace_mount(&mut manifest, &PodmanConfig::from_env().workspace_root);
    ensure_environment_dev_mount(&mut manifest, config)?;

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
    let parsed: container_plugin::sync::PluginsConfig = toml::from_str(&text)
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
            if ensure_literature_mount(&mut manifest, config) {
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

    let mut source = toml::to_string_pretty(manifest)
        .map_err(|e| Error::Other(format!("serialize {}: {e}", path.display())))?;
    if let Some(catalog_source) = catalog_source {
        source.push_str(catalog_source);
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(source.as_bytes())
        })
        .map_err(|e| Error::Other(format!("write {}: {e}", path.display())))
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
        path: "/literature".into(),
        backend: "literature".into(),
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
        path: "/literature".into(),
        backend: backend_id,
        source: "/".into(),
        read_only: false,
        permissions: Default::default(),
    });
    true
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
                path: "/".into(),
                backend: "default".into(),
                source: "/".into(),
                read_only: false,
                permissions: Default::default(),
            }],
        }
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
            .writing_db_path(directory.path().join("writing.db"))
            .app_db_path(directory.path().join("app.db"))
            .kms_db_path(directory.path().join("knowledge.db"))
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
            .writing_db_path(directory.path().join("writing.db"))
            .app_db_path(directory.path().join("app.db"))
            .kms_db_path(directory.path().join("knowledge.db"))
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
