//! Supported project devcontainer.json settings. All lifecycle commands run in the container.
use super::error::{GatewayError, GatewayResult};
use container_runtime::PodmanConfig;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::time::timeout;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DevConfig {
    pub image: String,
    pub build: Option<BuildConfig>,
    pub env: BTreeMap<String, String>,
    pub remote_env: BTreeMap<String, String>,
    pub container_user: Option<String>,
    pub remote_user: Option<String>,
    pub update_remote_uid: bool,
    // Each lifecycle stage may contain parallel commands.
    pub stages: Vec<Vec<Vec<String>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BuildConfig {
    dockerfile: PathBuf,
    context: PathBuf,
    args: BTreeMap<String, String>,
    target: Option<String>,
    cache_from: Vec<String>,
}

fn invalid(message: impl Into<String>) -> GatewayError {
    GatewayError::Message(message.into())
}

fn substitute(
    value: &str,
    workspace: &Path,
    container_env: Option<&BTreeMap<String, String>>,
) -> GatewayResult<String> {
    let mut result = String::new();
    let mut remaining = value;
    while let Some(start) = remaining.find("${") {
        result.push_str(&remaining[..start]);
        let after = &remaining[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| invalid("devcontainer.json 变量缺少右花括号"))?;
        let key = &after[..end];
        let resolved = match key {
            "localWorkspaceFolder" => workspace
                .to_str()
                .ok_or_else(|| invalid("工作区路径不是 UTF-8"))?
                .to_string(),
            "localWorkspaceFolderBasename" => workspace
                .file_name()
                .and_then(|v| v.to_str())
                .ok_or_else(|| invalid("工作区名称不是 UTF-8"))?
                .to_string(),
            "containerWorkspaceFolder" => "/work".to_string(),
            "containerWorkspaceFolderBasename" => "work".to_string(),
            _ if key.starts_with("localEnv:") => {
                let (name, default) = key[9..].split_once(':').unwrap_or((&key[9..], ""));
                std::env::var(name).unwrap_or_else(|_| default.to_string())
            }
            _ if key.starts_with("containerEnv:") && container_env.is_some() => {
                let (name, default) = key[13..].split_once(':').unwrap_or((&key[13..], ""));
                container_env
                    .and_then(|env| env.get(name))
                    .cloned()
                    .unwrap_or_else(|| default.to_string())
            }
            _ => return Err(invalid(format!("devcontainer.json 不支持变量 ${{{key}}}"))),
        };
        result.push_str(&resolved);
        remaining = &after[end + 1..];
    }
    result.push_str(remaining);
    if result.contains('\0') {
        return Err(invalid("devcontainer.json 值包含空字符"));
    }
    Ok(result)
}

fn command(value: &Value, workspace: &Path) -> GatewayResult<Vec<String>> {
    if let Some(text) = value.as_str() {
        return Ok(vec![
            "/bin/sh".into(),
            "-c".into(),
            substitute(text, workspace, None)?,
        ]);
    }
    let array = value
        .as_array()
        .ok_or_else(|| invalid("生命周期命令必须是字符串、字符串数组或命令对象"))?;
    if array.is_empty() {
        return Err(invalid("生命周期命令数组不能为空"));
    }
    array
        .iter()
        .map(|item| {
            substitute(
                item.as_str()
                    .ok_or_else(|| invalid("生命周期命令参数必须是字符串"))?,
                workspace,
                None,
            )
        })
        .collect()
}

impl DevConfig {
    pub fn load(workspace: &Path, default_image: &str) -> GatewayResult<Self> {
        let mut config = Self {
            image: default_image.to_owned(),
            build: None,
            env: BTreeMap::new(),
            remote_env: BTreeMap::new(),
            container_user: None,
            remote_user: None,
            update_remote_uid: true,
            stages: vec![vec![]; 5],
        };
        let candidates = [
            workspace.join(".devcontainer/devcontainer.json"),
            workspace.join(".devcontainer.json"),
        ];
        let mut selected = None;
        for path in &candidates {
            match std::fs::symlink_metadata(path) {
                Ok(_) => {
                    selected = Some(path);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(invalid(format!("无法读取 devcontainer.json：{error}"))),
            }
        }
        let Some(path) = selected else {
            return Ok(config);
        };
        let root = workspace
            .canonicalize()
            .map_err(|e| invalid(format!("无法读取项目目录：{e}")))?;
        let path = path
            .canonicalize()
            .map_err(|e| invalid(format!("无法读取 devcontainer.json：{e}")))?;
        if !path.starts_with(&root) {
            return Err(invalid("devcontainer.json 必须位于项目目录内"));
        }
        if std::fs::metadata(&path)
            .map_err(|e| invalid(e.to_string()))?
            .len()
            > 1024 * 1024
        {
            return Err(invalid("devcontainer.json 超过 1 MiB"));
        }
        let contents = std::fs::read_to_string(&path)
            .map_err(|e| invalid(format!("无法读取 devcontainer.json：{e}")))?;
        let value: Value = json5::from_str(&contents)
            .map_err(|e| invalid(format!("devcontainer.json 格式错误：{e}")))?;
        let obj = value
            .as_object()
            .ok_or_else(|| invalid("devcontainer.json 必须是对象"))?;
        for key in obj.keys() {
            if ![
                "$schema",
                "name",
                "image",
                "build",
                "dockerFile",
                "context",
                "containerEnv",
                "remoteEnv",
                "containerUser",
                "remoteUser",
                "updateRemoteUserUID",
                "workspaceFolder",
                "overrideCommand",
                "onCreateCommand",
                "updateContentCommand",
                "postCreateCommand",
                "postStartCommand",
                "postAttachCommand",
            ]
            .contains(&key.as_str())
            {
                return Err(invalid(format!("devcontainer.json 的 {key} 暂不支持")));
            }
        }
        if let Some(folder) = obj.get("workspaceFolder") {
            if folder
                .as_str()
                .map(|v| substitute(v, workspace, None))
                .transpose()?
                .as_deref()
                != Some("/work")
            {
                return Err(invalid("目前仅支持 /work 作为 workspaceFolder"));
            }
        }
        if let Some(override_command) = obj.get("overrideCommand") {
            if override_command.as_bool() != Some(true) {
                return Err(invalid("目前仅支持 overrideCommand: true"));
            }
        }
        if let Some(image) = obj.get("image") {
            config.image = image
                .as_str()
                .filter(|v| !v.is_empty() && !v.starts_with('-') && !v.contains('\0'))
                .ok_or_else(|| invalid("devcontainer.json 镜像无效"))?
                .to_owned();
        }
        for (key, target) in [
            ("containerUser", &mut config.container_user),
            ("remoteUser", &mut config.remote_user),
        ] {
            if let Some(value) = obj.get(key) {
                let user = substitute(
                    value
                        .as_str()
                        .ok_or_else(|| invalid(format!("{key} 必须是字符串")))?,
                    workspace,
                    None,
                )?;
                if user.is_empty() || user.starts_with('-') || user.contains(['\0', '\n', '\r']) {
                    return Err(invalid(format!("{key} 无效")));
                }
                *target = Some(user);
            }
        }
        if let Some(value) = obj.get("updateRemoteUserUID") {
            config.update_remote_uid = value
                .as_bool()
                .ok_or_else(|| invalid("updateRemoteUserUID 必须是布尔值"))?;
        }
        if obj.contains_key("build") || obj.contains_key("dockerFile") {
            if obj.contains_key("image") {
                return Err(invalid(
                    "devcontainer.json 不能同时指定 image 和 Dockerfile 构建",
                ));
            }
            let build = match obj.get("build") {
                Some(value) => value
                    .as_object()
                    .ok_or_else(|| invalid("build 必须是对象"))?,
                None => {
                    static EMPTY: std::sync::LazyLock<serde_json::Map<String, Value>> =
                        std::sync::LazyLock::new(serde_json::Map::new);
                    &EMPTY
                }
            };
            for key in build.keys() {
                if !["dockerfile", "context", "args", "target", "cacheFrom"].contains(&key.as_str())
                {
                    return Err(invalid(format!(
                        "devcontainer.json 的 build.{key} 暂不支持"
                    )));
                }
            }
            if obj.contains_key("dockerFile") && build.contains_key("dockerfile") {
                return Err(invalid("请勿同时指定 dockerFile 和 build.dockerfile"));
            }
            if obj.contains_key("context") && build.contains_key("context") {
                return Err(invalid("请勿同时指定 context 和 build.context"));
            }
            let base = path.parent().ok_or_else(|| invalid("无法确定配置目录"))?;
            let resolve = |value: &str| -> GatewayResult<PathBuf> {
                let resolved = base
                    .join(substitute(value, workspace, None)?)
                    .canonicalize()
                    .map_err(|e| invalid(format!("无法读取构建路径：{e}")))?;
                if !resolved.starts_with(&root) {
                    return Err(invalid("构建路径必须位于项目目录内"));
                }
                Ok(resolved)
            };
            let dockerfile = obj
                .get("dockerFile")
                .or_else(|| build.get("dockerfile"))
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("Dockerfile 构建需要 build.dockerfile"))?;
            let dockerfile = resolve(dockerfile)?;
            if !dockerfile.is_file() {
                return Err(invalid("Dockerfile 必须是文件"));
            }
            let context = obj
                .get("context")
                .or_else(|| build.get("context"))
                .map(|v| {
                    v.as_str()
                        .ok_or_else(|| invalid("build.context 必须是字符串"))
                })
                .transpose()?
                .unwrap_or(".");
            let context = resolve(context)?;
            if !context.is_dir() {
                return Err(invalid("构建上下文必须是目录"));
            }
            let mut args = BTreeMap::new();
            if let Some(value) = build.get("args") {
                for (name, value) in value
                    .as_object()
                    .ok_or_else(|| invalid("build.args 必须是对象"))?
                {
                    if name.is_empty() || name.contains(['=', '\0']) || name.starts_with('-') {
                        return Err(invalid("构建参数名无效"));
                    }
                    args.insert(
                        name.clone(),
                        substitute(
                            value
                                .as_str()
                                .ok_or_else(|| invalid("构建参数值必须是字符串"))?,
                            workspace,
                            None,
                        )?,
                    );
                }
            }
            let target = build
                .get("target")
                .map(|v| {
                    v.as_str()
                        .filter(|s| !s.is_empty() && !s.contains('\0'))
                        .ok_or_else(|| invalid("build.target 无效"))
                        .map(str::to_owned)
                })
                .transpose()?;
            let cache_from = match build.get("cacheFrom") {
                None => Vec::new(),
                Some(Value::String(value)) => vec![value.clone()],
                Some(Value::Array(values)) => values
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| invalid("build.cacheFrom 必须是字符串或字符串数组"))
                    })
                    .collect::<GatewayResult<Vec<_>>>()?,
                Some(_) => return Err(invalid("build.cacheFrom 必须是字符串或字符串数组")),
            };
            if cache_from.iter().any(|s| s.is_empty() || s.contains('\0')) {
                return Err(invalid("build.cacheFrom 无效"));
            }
            config.build = Some(BuildConfig {
                dockerfile,
                context,
                args,
                target,
                cache_from,
            });
        } else if obj.contains_key("context") {
            return Err(invalid("context 需要 dockerFile"));
        }
        if let Some(env) = obj.get("containerEnv") {
            for (key, value) in env
                .as_object()
                .ok_or_else(|| invalid("containerEnv 必须是对象"))?
            {
                if key.is_empty() || key.contains(['=', '\0']) {
                    return Err(invalid("containerEnv 变量名无效"));
                }
                config.env.insert(
                    key.clone(),
                    substitute(
                        value
                            .as_str()
                            .ok_or_else(|| invalid("containerEnv 变量值必须是字符串"))?,
                        workspace,
                        None,
                    )?,
                );
            }
        }
        if let Some(env) = obj.get("remoteEnv") {
            for (key, value) in env
                .as_object()
                .ok_or_else(|| invalid("remoteEnv 必须是对象"))?
            {
                if key.is_empty() || key.contains(['=', '\0']) {
                    return Err(invalid("remoteEnv 变量名无效"));
                }
                let value = value
                    .as_str()
                    .ok_or_else(|| invalid("remoteEnv 变量值必须是字符串"))?;
                // Validate syntax and supported variables without resolving image environment yet.
                substitute(value, workspace, Some(&BTreeMap::new()))?;
                config.remote_env.insert(key.clone(), value.to_owned());
            }
        }
        for (index, key) in [
            "onCreateCommand",
            "updateContentCommand",
            "postCreateCommand",
            "postStartCommand",
            "postAttachCommand",
        ]
        .iter()
        .enumerate()
        {
            if let Some(value) = obj.get(*key) {
                config.stages[index] = if let Some(group) = value.as_object() {
                    group
                        .values()
                        .map(|item| command(item, workspace))
                        .collect::<GatewayResult<Vec<_>>>()?
                } else {
                    vec![command(value, workspace)?]
                };
            }
        }
        Ok(config)
    }

    pub async fn build_image(&self, podman: &PodmanConfig) -> GatewayResult<String> {
        let Some(build) = &self.build else {
            return Ok(self.image.clone());
        };
        let mut command = podman.async_command();
        command
            .args(["build", "--quiet", "--file"])
            .arg(&build.dockerfile);
        for (name, value) in &build.args {
            command.arg(format!("--build-arg={name}={value}"));
        }
        if let Some(target) = &build.target {
            command.arg(format!("--target={target}"));
        }
        for image in &build.cache_from {
            command.arg(format!("--cache-from={image}"));
        }
        let output = timeout(
            Duration::from_secs(600),
            command
                .arg(&build.context)
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| invalid("开发容器镜像构建超时"))?
        .map_err(|e| invalid(format!("无法构建开发容器镜像：{e}")))?;
        if !output.status.success() {
            return Err(invalid(format!(
                "开发容器镜像构建失败：{}",
                String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(4000)
                    .collect::<String>()
            )));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let image = text.lines().last().unwrap_or("").trim();
        let hex = image.strip_prefix("sha256:").unwrap_or(image);
        if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid("构建完成但未返回有效镜像 ID"));
        }
        Ok(image.to_owned())
    }

    /// Resolve the target user's image UID/GID before creating the workspace mount.
    pub async fn user_settings(
        &self,
        podman: &PodmanConfig,
        image: &str,
    ) -> GatewayResult<(Option<String>, Option<String>)> {
        let Some(user) = self.remote_user.as_ref().or(self.container_user.as_ref()) else {
            return Ok((None, None));
        };
        let (name, group) = user.split_once(':').unwrap_or((user.as_str(), ""));
        if name.is_empty()
            || (!group.is_empty() && !group.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Err(invalid("用户组目前只支持数字 GID"));
        }
        let output = timeout(Duration::from_secs(120), podman.async_command().args(["run", "--rm", "--pull=missing", "--network=none", "--user=0", "--entrypoint=/bin/sh", image, "-c", "id -u \"$1\" && { if [ -n \"$2\" ]; then printf '%s\\n' \"$2\"; else id -g \"$1\"; fi; }", "user-probe", name, group]).stdin(Stdio::null()).kill_on_drop(true).output()).await.map_err(|_| invalid("检查容器用户超时"))?.map_err(|e| invalid(format!("无法检查容器用户：{e}")))?;
        if !output.status.success() {
            return Err(invalid(format!("容器中不存在用户 {user}")));
        }
        let info = String::from_utf8_lossy(&output.stdout);
        let mut lines = info.lines();
        let uid = lines
            .next()
            .and_then(|v| v.parse::<u32>().ok())
            .ok_or_else(|| invalid("无法读取容器用户 UID"))?;
        let gid = lines
            .next()
            .and_then(|v| v.parse::<u32>().ok())
            .ok_or_else(|| invalid("无法读取容器用户 GID"))?;
        let namespace = self
            .update_remote_uid
            .then(|| format!("--userns=keep-id:uid={uid},gid={gid}"));
        let container_user = if let Some(user) = &self.container_user {
            Some(user.clone())
        } else {
            let output = timeout(
                Duration::from_secs(15),
                podman
                    .async_command()
                    .args(["image", "inspect", "--format={{.Config.User}}", image])
                    .stdin(Stdio::null())
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .map_err(|_| invalid("检查镜像用户超时"))?
            .map_err(|e| invalid(format!("无法检查镜像用户：{e}")))?;
            if !output.status.success() {
                return Err(invalid("无法检查镜像用户"));
            }
            let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            Some(if value.is_empty() {
                "0".to_string()
            } else {
                value
            })
        };
        Ok((namespace, container_user))
    }

    pub async fn resolve_remote_env(
        &self,
        podman: &PodmanConfig,
        container: &str,
        workspace: &Path,
    ) -> GatewayResult<BTreeMap<String, String>> {
        if self.remote_env.is_empty() {
            return Ok(BTreeMap::new());
        }
        let output = timeout(
            Duration::from_secs(15),
            podman
                .async_command()
                .args([
                    "inspect",
                    "--type=container",
                    "--format={{json .Config.Env}}",
                    container,
                ])
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| invalid("读取容器环境超时"))?
        .map_err(|e| invalid(format!("无法读取容器环境：{e}")))?;
        if !output.status.success() {
            return Err(invalid("无法读取容器环境"));
        }
        let entries: Option<Vec<String>> = serde_json::from_slice(&output.stdout)
            .map_err(|e| invalid(format!("容器环境格式错误：{e}")))?;
        let env: BTreeMap<String, String> = entries
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| {
                item.split_once('=')
                    .map(|(key, value)| (key.to_owned(), value.to_owned()))
            })
            .collect();
        self.remote_env
            .iter()
            .map(|(key, value)| Ok((key.clone(), substitute(value, workspace, Some(&env))?)))
            .collect()
    }

    pub async fn run_stages(
        &self,
        podman: &PodmanConfig,
        container: &str,
        start: usize,
        remote_env: &BTreeMap<String, String>,
        remote_user: Option<&str>,
    ) -> GatewayResult<()> {
        for stage in &self.stages[start..] {
            let tasks = stage.iter().map(|args| async move {
                let output = timeout(Duration::from_secs(120), async move {
                    let mut command = podman.async_command();
                    command.args(["exec", "--workdir=/work"]);
                    if let Some(user) = remote_user {
                        command.arg(format!("--user={user}"));
                    }
                    command
                        .args(
                            remote_env
                                .iter()
                                .map(|(key, value)| format!("--env={key}={value}")),
                        )
                        .arg(container)
                        .args(args)
                        .stdin(Stdio::null())
                        .kill_on_drop(true);
                    command.output().await
                })
                .await
                .map_err(|_| invalid("开发容器生命周期命令超时"))?
                .map_err(|e| invalid(format!("无法执行开发容器生命周期命令：{e}")))?;
                if !output.status.success() {
                    return Err(invalid(format!(
                        "开发容器生命周期命令失败（{}）：{}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr)
                            .chars()
                            .take(2000)
                            .collect::<String>()
                    )));
                }
                Ok(())
            });
            futures::future::try_join_all(tasks).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonc_and_rejects_unsupported_runtime_settings() {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join(".devcontainer.json");
        std::fs::write(&path, r#"{
            // JSON with comments and a trailing comma is accepted.
            "image": "example/dev:latest",
            "containerEnv": { "WORK": "${containerWorkspaceFolder}", "FALLBACK": "${localEnv:AUTONOMICS_MISSING_DEV_TEST:default}" },
            "postCreateCommand": ["printf", "literal argument"],
        }"#).unwrap();
        let parsed = DevConfig::load(workspace.path(), "fallback")
            .unwrap_or_else(|_| panic!("parse config"));
        assert_eq!(parsed.image, "example/dev:latest");
        assert_eq!(parsed.env["WORK"], "/work");
        assert_eq!(parsed.env["FALLBACK"], "default");
        assert_eq!(parsed.stages[2][0], ["printf", "literal argument"]);
        std::fs::write(&path, r#"{"features": {"example": {}}}"#).unwrap();
        assert!(DevConfig::load(workspace.path(), "fallback").is_err());
        std::fs::write(
            &path,
            r#"{"containerEnv": {"FOO": "${containerEnv:PATH}"}}"#,
        )
        .unwrap();
        assert!(DevConfig::load(workspace.path(), "fallback").is_err());
    }

    #[test]
    fn remote_variables_use_container_environment_and_defaults() {
        let env = BTreeMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string())]);
        let value = substitute(
            "${containerEnv:PATH}:/tools:${containerEnv:MISSING:fallback}",
            Path::new("/tmp/project"),
            Some(&env),
        )
        .unwrap_or_else(|_| panic!("resolve remote environment"));
        assert_eq!(value, "/usr/bin:/bin:/tools:fallback");
        assert!(substitute("${containerEnv:PATH}", Path::new("/tmp/project"), None).is_err());
    }

    #[test]
    fn parses_dockerfile_build_paths_and_options() {
        let workspace = tempfile::tempdir().unwrap();
        let folder = workspace.path().join(".devcontainer");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("Dockerfile"), "FROM scratch\n").unwrap();
        std::fs::write(folder.join("devcontainer.json"), r#"{"build":{"dockerfile":"Dockerfile","context":"..","args":{"FOO":"${containerWorkspaceFolder}"},"target":"dev","cacheFrom":["local:cache"]}}"#).unwrap();
        let config =
            DevConfig::load(workspace.path(), "unused").unwrap_or_else(|_| panic!("build config"));
        let build = config.build.unwrap();
        assert_eq!(build.context, workspace.path().canonicalize().unwrap());
        assert_eq!(
            build.dockerfile,
            folder.join("Dockerfile").canonicalize().unwrap()
        );
        assert_eq!(build.args["FOO"], "/work");
        assert_eq!(build.target.as_deref(), Some("dev"));
        assert_eq!(build.cache_from, ["local:cache"]);
        std::fs::write(
            folder.join("devcontainer.json"),
            r#"{"build":{"dockerfile":"Dockerfile","options":["--network=host"]}}"#,
        )
        .unwrap();
        assert!(DevConfig::load(workspace.path(), "unused").is_err());
        std::fs::write(
            folder.join("devcontainer.json"),
            r#"{"build":{"dockerfile":"Dockerfile","context":"../.."}}"#,
        )
        .unwrap();
        assert!(DevConfig::load(workspace.path(), "unused").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_configuration_symlink_outside_project() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), workspace.path().join(".devcontainer.json"))
            .unwrap();
        assert!(DevConfig::load(workspace.path(), "fallback").is_err());
        std::fs::remove_file(outside.path()).unwrap();
        assert!(DevConfig::load(workspace.path(), "fallback").is_err());
    }
}
