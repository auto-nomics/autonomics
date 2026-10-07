use std::{path::Path, sync::Arc};

pub fn configure_plugin_vfs(state_dir: &Path) {
    let data_root = state_dir.join("vfs-data");
    let layout = plugin_rsi::PluginStateLayout::v2(state_dir);
    layout.ensure_v2_directories().unwrap();
    let plugin_root = layout.workspace_root();
    std::fs::create_dir_all(&data_root).unwrap();
    std::fs::create_dir_all(&plugin_root).unwrap();

    let deny = |path: &str| vfs::permission::VfsPathRule::Deny {
        path: path.to_string(),
        access: vec![
            vfs::permission::VfsAccess::Read,
            vfs::permission::VfsAccess::Write,
            vfs::permission::VfsAccess::Execute,
        ],
    };
    let manifest = vfs::VfsManifest {
        backend: vec![vfs::BackendDefinition {
            id: "plugin-development".into(),
            config: vfs::BackendConfig::local(plugin_root.to_string_lossy().into_owned()),
        }],
        mount: vec![vfs::MountDefinition {
            path: plugin_rsi::PLUGIN_DEVELOPMENT_VFS_ROOT.into(),
            backend: "plugin-development".into(),
            source: "/".into(),
            read_only: false,
            permissions: vfs::permission::MountPermissions::unix(
                vfs::permission::VfsOwnership {
                    uid: vfs::permission::VFS_ROOT_UID,
                    gid: vfs::permission::VFS_PLUGIN_DEVELOPER_GID,
                },
                vfs::permission::VfsMode::from_bits(0o775),
                vfs::permission::VfsMode::from_bits(0o664),
                vfs::permission::VfsMode::from_bits(0o775),
            )
            .with_rules(vec![
                deny(".git"),
                deny(".git/**"),
                deny("**/.git"),
                deny("**/.git/**"),
                vfs::permission::VfsPathRule::Deny {
                    path: ".autonomics-layout.toml".into(),
                    access: vec![
                        vfs::permission::VfsAccess::Read,
                        vfs::permission::VfsAccess::Write,
                        vfs::permission::VfsAccess::Execute,
                    ],
                },
                vfs::permission::VfsPathRule::Deny {
                    path: "*/manifest.toml".into(),
                    access: vec![vfs::permission::VfsAccess::Write],
                },
            ]),
        }],
    };
    let mounts = Arc::new(vfs::MountedObjectStore::from_manifest(&manifest).unwrap());
    let storage = vfs::OpendalFileStorage::with_mounts(&data_root, mounts);
    plugin_rsi::PluginDevelopmentToolsetRegistry::global()
        .configure_vfs(storage, plugin_root)
        .unwrap();
}
