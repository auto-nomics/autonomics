//! 数据目录解析：与 TUI 共享 `~/.autonomics`（尊重既有 `AUTONOMICS_*` env）。
//!
//! 与 `runtime::RuntimeConfig`（crates/runtime/src/config.rs:247）的解析
//! 语义对齐，但不依赖 runtime crate；唯一差异是 `data_dir` 的默认值——
//! TUI 默认硬编码开发机路径 `/mnt/disk3/test`（config.rs:50），桌面版
//! 默认落到共享 state dir 之下（env 已设置时两者一致）。

use std::path::PathBuf;

/// 桌面壳需要的全部文件系统位置。
#[derive(Debug, Clone)]
pub struct Paths {
    pub state_dir: PathBuf,
    pub bib_db: PathBuf,
    pub app_db: PathBuf,
    pub data_dir: PathBuf,
    pub literature_root: PathBuf,
    pub vfs_manifest: PathBuf,
    pub logs_dir: PathBuf,
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

pub fn resolve() -> Result<Paths, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME 未设置，无法定位默认数据目录 ~/.autonomics".to_string())?;

    let state_dir =
        env_path("AUTONOMICS_STATE_DIR").unwrap_or_else(|| home.join(".autonomics"));
    let bib_db =
        env_path("AUTONOMICS_BIB_DB").unwrap_or_else(|| state_dir.join("bib.db"));
    let app_db =
        env_path("AUTONOMICS_APP_DB").unwrap_or_else(|| state_dir.join("config.db"));
    let data_dir =
        env_path("AUTONOMICS_DATA_DIR").unwrap_or_else(|| state_dir.join("data"));

    Ok(Paths {
        literature_root: state_dir.join("literature"),
        vfs_manifest: state_dir.join("vfs.toml"),
        logs_dir: state_dir.join("logs"),
        state_dir,
        bib_db,
        app_db,
        data_dir,
    })
}

impl Paths {
    /// 启动前确保目录存在（文件本身由各子系统创建）。
    pub fn ensure_dirs(&self) -> Result<(), String> {
        for dir in [
            &self.state_dir,
            &self.data_dir,
            &self.literature_root,
            &self.logs_dir,
        ] {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("创建目录 {} 失败：{e}", dir.display()))?;
        }
        Ok(())
    }
}
