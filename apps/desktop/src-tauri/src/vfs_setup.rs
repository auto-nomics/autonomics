//! vfs.toml 载入/落盘 + `OpendalFileStorage` 构造。
//!
//! 语义复刻自 `crates/runtime/src/host.rs`（load_or_create :544 / write :574 /
//! extract_catalog_section :613 / default_vfs_manifest :692 /
//! ensure_literature_mount :739），与 TUI 共享同一个 `state_dir/vfs.toml`。
//! 复刻而非把 runtime 的函数改 pub——那会让桌面壳编译期拖入
//! data-engine/container/bollard 依赖树。改动这里时请同步 host.rs。

use std::path::Path;
use std::sync::Arc;

use vfs::{
    BackendConfig, BackendDefinition, MountedObjectStore, MountDefinition, OpendalFileStorage,
    VfsManifest,
};

use crate::paths::Paths;

/// 读（或首次创建）`state_dir/vfs.toml`，返回可用的文件存储。
pub fn build_file_storage(paths: &Paths) -> Result<Arc<OpendalFileStorage>, String> {
    let manifest = load_or_create(paths)?;
    let mounts = Arc::new(
        MountedObjectStore::from_manifest(&manifest)
            .map_err(|e| format!("vfs.toml 挂载配置无效：{e}"))?,
    );
    Ok(Arc::new(OpendalFileStorage::with_mounts(
        &paths.data_dir,
        mounts,
    )))
}

/// host.rs `load_or_create_vfs_manifest` 的镜像（:544-572）。
fn load_or_create(paths: &Paths) -> Result<VfsManifest, String> {
    let manifest_path: &Path = &paths.vfs_manifest;
    match std::fs::read_to_string(manifest_path) {
        Ok(source) => {
            let catalog_source = extract_catalog_section(&source);
            let mut manifest = VfsManifest::from_toml(&source)
                .map_err(|e| format!("解析 {} 失败：{e}", manifest_path.display()))?;
            if ensure_literature_mount(&mut manifest, paths) {
                write_vfs_manifest(manifest_path, &manifest, catalog_source.as_deref())?;
            }
            Ok(manifest)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let manifest = default_vfs_manifest(paths);
            write_vfs_manifest(manifest_path, &manifest, None)?;
            Ok(manifest)
        }
        Err(e) => Err(format!("读取 {} 失败：{e}", manifest_path.display())),
    }
}

/// host.rs `write_vfs_manifest` 的镜像（:574-611），0600 权限。
fn write_vfs_manifest(
    path: &Path,
    manifest: &VfsManifest,
    catalog_source: Option<&str>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 {} 失败：{e}", parent.display()))?;
    }

    let mut source = toml::to_string_pretty(manifest)
        .map_err(|e| format!("序列化 {} 失败：{e}", path.display()))?;
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
        .map_err(|e| format!("写入 {} 失败：{e}", path.display()))
}

/// host.rs `extract_catalog_section` 的镜像（:613-627）：回写时保留
/// `[catalog]` 段原文，防丢 runtime 的目录配置。
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

/// host.rs `default_vfs_manifest` 的镜像（:692-735）。
fn default_vfs_manifest(paths: &Paths) -> VfsManifest {
    let mut backend = vec![BackendDefinition {
        id: "default".into(),
        config: BackendConfig::local("/"),
    }];
    let mut mount = vec![MountDefinition {
        path: "/".into(),
        backend: "default".into(),
        source: paths.data_dir.to_string_lossy().to_string(),
        read_only: false,
    }];
    backend.push(BackendDefinition {
        id: "literature".into(),
        config: BackendConfig::local(paths.literature_root.to_string_lossy().to_string()),
    });
    mount.push(MountDefinition {
        path: "/literature".into(),
        backend: "literature".into(),
        source: "/".into(),
        read_only: false,
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
            config: BackendConfig::oss(bucket, endpoint, ak, sk),
        });
        mount.push(MountDefinition {
            path: "/data/oss".into(),
            backend: "oss-prod".into(),
            source: "/".into(),
            read_only: true,
        });
    }

    VfsManifest { backend, mount }
}

/// host.rs `ensure_literature_mount` 的镜像（:739-773）：给缺 `/literature`
/// 挂载的旧 manifest 补齐（backend id 冲突时加 `_` 后缀）。
fn ensure_literature_mount(manifest: &mut VfsManifest, paths: &Paths) -> bool {
    if manifest.mount.iter().any(|m| m.path == "/literature") {
        return false;
    }

    let mut backend_id = "literature".to_owned();
    while manifest.backend.iter().any(|b| b.id == backend_id) {
        backend_id.push('_');
    }
    manifest.backend.push(BackendDefinition {
        id: backend_id.clone(),
        config: BackendConfig::local(paths.literature_root.to_string_lossy().to_string()),
    });
    manifest.mount.push(MountDefinition {
        path: "/literature".into(),
        backend: backend_id,
        source: "/".into(),
        read_only: false,
    });
    true
}
