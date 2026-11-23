//! Workspace path validation and file operations executed inside the container.

use std::path::{Component, Path};

use axum::http::StatusCode;

use super::super::error::{GatewayError, GatewayResult};

pub(crate) fn validate_file_path(path: &str) -> GatewayResult<()> {
    if path.is_empty()
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || !Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(name) if name != ".git"))
    {
        return Err(GatewayError::Status(
            StatusCode::BAD_REQUEST,
            "无效的项目文件路径".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_directory_move(from: &str, to: &str) -> GatewayResult<()> {
    validate_file_path(from)?;
    validate_file_path(to)?;
    if Path::new(to).starts_with(Path::new(from)) {
        return Err(GatewayError::Message(
            "不能将文件夹移动到自身或其子目录".into(),
        ));
    }
    Ok(())
}

/// Verify each path component inside the mounted workspace before access.
/// The caller validates the relative path; the shell receives it as argv.
pub(crate) const CHECK_PATH: &str = r#"
set -eu
path=$1
rest=$path
current=/work
while :; do
  part=${rest%%/*}
  current=$current/$part
  if [ -L "$current" ]; then echo '不允许访问符号链接' >&2; exit 2; fi
  case "$rest" in */*) rest=${rest#*/} ;; *) break ;; esac
done
"#;

pub(crate) const LIST_SCRIPT: &str = r#"set -eu
cd /work
find . \( -name .git -o -name .venv \) -prune -o -type f -print0"#;

pub(crate) const LIST_DIRECTORIES_SCRIPT: &str = r#"set -eu
cd /work
find . \( -name .git -o -name .venv \) -prune -o -type d -print0"#;

// A portable, NUL-delimited snapshot made inside the development container.
// Dependency and VCS directories are excluded to keep periodic scans bounded.
pub(crate) const SNAPSHOT_SCRIPT: &str = r#"set -eu
cd /work
if find . -maxdepth 0 -printf '' >/dev/null 2>&1; then
  find . \( -name .git -o -name .venv -o -name node_modules \) -prune -o \( -type f -o -type d \) -printf '%y\0%p\0%T@\0%s\0'
else
  find . \( -name .git -o -name .venv -o -name node_modules \) -prune -o \( -type f -o -type d \) -exec sh -c '
    for file do
      if [ -d "$file" ]; then kind=d; else kind=f; fi
      metadata=$(stat -c "%Y:%s" "$file") || exit 1
      printf "%s\0%s\0%s\0%s\0" "$kind" "$file" "$metadata" ""
    done
  ' sh {} +
fi"#;

pub(crate) fn read_script() -> String {
    format!(
        "{}\n[ -f \"$current\" ] || {{ echo '文件不存在' >&2; exit 2; }}\n[ \"$(wc -c < \"$current\")\" -le 1048576 ] || {{ echo '文件超过大小限制' >&2; exit 2; }}\ncat -- \"$current\"",
        CHECK_PATH
    )
}

pub(crate) fn create_script() -> String {
    format!(
        r#"{}
[ ! -e "$current" ] || exit 42
(set -C; : > "$current") || {{ echo '无法创建文件' >&2; exit 2; }}
"#,
        CHECK_PATH
    )
}

pub(crate) fn create_directory_script() -> String {
    format!(
        r#"{}
[ ! -e "$current" ] || exit 42
mkdir -- "$current"
"#,
        CHECK_PATH
    )
}

pub(crate) fn rename_directory_script() -> String {
    format!(
        r#"{}
source=$current
[ -d "$source" ] || {{ echo '文件夹不存在' >&2; exit 2; }}
set -- "$2"
{}
[ ! -e "$current" ] || exit 42
mv -T -n -- "$source" "$current"
[ ! -e "$source" ] || exit 42
"#,
        CHECK_PATH, CHECK_PATH
    )
}

pub(crate) fn delete_directory_script() -> String {
    format!(
        r#"{}
[ -d "$current" ] || {{ echo '文件夹不存在' >&2; exit 2; }}
rm -r -- "$current"
"#,
        CHECK_PATH
    )
}

pub(crate) fn delete_script() -> String {
    format!(
        r#"{}
[ -f "$current" ] || {{ echo '文件不存在' >&2; exit 2; }}
rm -- "$current"
"#,
        CHECK_PATH
    )
}

pub(crate) fn rename_script() -> String {
    format!(
        r#"{}
source=$current
[ -f "$source" ] || {{ echo '文件不存在' >&2; exit 2; }}
set -- "$2"
{}
[ ! -e "$current" ] || exit 42
ln -- "$source" "$current" || {{ echo '无法重命名文件' >&2; exit 2; }}
if ! rm -- "$source"; then rm -f -- "$current"; echo '无法重命名文件' >&2; exit 2; fi
"#,
        CHECK_PATH, CHECK_PATH
    )
}

pub(crate) fn save_script() -> String {
    format!(
        r#"{}
[ -f "$current" ] || {{ echo '文件不存在' >&2; exit 2; }}
expected=$(mktemp /tmp/autonomics-expected.XXXXXXXX)
temp=$(mktemp "$(dirname "$current")/.autonomics-edit.XXXXXXXX")
trap 'rm -f "$expected" "$temp"' EXIT HUP INT TERM
dd bs=1 count="$2" of="$expected" 2>/dev/null
cat > "$temp"
cmp -s "$current" "$expected" || exit 42
chmod "$(stat -c %a "$current")" "$temp"
mv -f "$temp" "$current"
"#,
        CHECK_PATH
    )
}
