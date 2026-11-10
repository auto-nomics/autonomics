//! Enrollment connection lifecycle and step diagnostics. Authentication is
//! reused through the process-wide OpenSSH master policy; this object does
//! not retain a remote shell or retry operations with side effects.
use super::enroll::{EnrollProgress, shq, valid_sha256};

pub(super) struct ClusterConnection {
    id: String,
    alias: String,
    program: String,
    python: String,
    ssh: dag_core::SshTarget,
    progress: EnrollProgress,
}

impl ClusterConnection {
    pub fn new(id: &str, alias: &str, program: &str, python: &str) -> Self {
        tracing::info!(
            target = id,
            alias,
            program,
            python,
            multiplex = vfs::ssh_channel::is_configured(),
            batch_mode = vfs::ssh_channel::batch_mode_option(),
            "cluster connection configured"
        );
        Self {
            id: id.into(),
            alias: alias.into(),
            program: program.into(),
            python: python.into(),
            ssh: dag_core::SshTarget::new(program, alias),
            progress: EnrollProgress::default(),
        }
    }

    pub fn with_progress(mut self, progress: EnrollProgress) -> Self {
        progress.report(
            "connection",
            &format!(
                "连接目标 {}；SSH={}；连接复用={}",
                self.alias,
                self.program,
                vfs::ssh_channel::is_configured()
            ),
        );
        self.progress = progress;
        self
    }

    async fn run(&self, stage: &str, script: &str) -> Result<std::process::Output, String> {
        let label = match stage {
            "authenticate" => "正在连接集群并验证 SSH 身份",
            "prepare_directory" => "正在准备远端 worker 目录",
            "check_python" => "正在检查远端 Python",
            "chmod" => "正在设置 worker 执行权限",
            "probe_worker" => "正在校验 worker 可执行文件和 SHA256",
            _ => stage,
        };
        self.progress.report(stage, label);
        let started = std::time::Instant::now();
        tracing::info!(target = %self.id, alias = %self.alias, stage, "cluster step started");
        let observe = |stream: &str, text: &str| {
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                self.progress.report(stage, &format!("{stream}: {line}"));
            }
        };
        let output = self.ssh.run_script_once_with_output(script, &observe).await.map_err(|error| {
            tracing::error!(target = %self.id, alias = %self.alias, stage, %error, "cluster step failed");
            format!("cluster {} step {stage}: {error}", self.alias)
        })?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::info!(target = %self.id, alias = %self.alias, stage,
            elapsed_ms = started.elapsed().as_millis() as u64,
            status = %output.status, stdout = %stdout.trim(), stderr = %stderr.trim(),
            "cluster step finished");
        if !output.status.success() {
            return Err(format!(
                "cluster {} step {stage} failed ({}); stdout: {}; stderr: {}",
                self.alias,
                output.status,
                stdout.trim(),
                stderr.trim()
            ));
        }
        self.progress.report(
            stage,
            &format!("完成（{} ms）", started.elapsed().as_millis()),
        );
        Ok(output)
    }

    pub async fn authenticate(&self) -> Result<(), String> {
        self.run("authenticate", "printf 'AUTONOMICS_CONNECTED\n'")
            .await?;
        Ok(())
    }

    pub async fn probe_worker(&self, path: &str) -> Result<String, String> {
        let output = self
            .run(
                "probe_worker",
                &format!("test -x {0} && sha256sum {0}", shq(path)),
            )
            .await?;
        let digest = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !valid_sha256(&digest) {
            return Err("worker probe returned an unparsable digest".into());
        }
        Ok(digest)
    }

    pub async fn verify_worker_identity(&self, path: &str, sha256: &str) -> Result<(), String> {
        self.progress
            .report("probe_worker", "正在验证独立 worker 协议和源码版本");
        let output = self
            .run("probe_worker", &format!("{} identity", shq(path)))
            .await.map_err(|error| format!(
                "{error}; deploy the standalone target/release/autonomics-worker, built from the same source revision as the gateway"
            ))?;
        let identity: dag_core::dag::execution::typed_worker::WorkerIdentity =
            serde_json::from_slice(&output.stdout).map_err(|error| format!(
                "invalid worker identity: {error}; deploy target/release/autonomics-worker, built from the same source revision as the gateway"
            ))?;
        identity
            .validate(sha256)
            .map_err(|error| error.to_string())?;
        self.progress.report(
            "probe_worker",
            &format!(
                "独立 worker 已就绪；协议={}；源码={}",
                identity.protocol, identity.source_revision
            ),
        );
        Ok(())
    }

    pub async fn deploy_worker(&self, local: &str, destination: &str) -> Result<(), String> {
        let local = std::path::Path::new(local);
        let metadata = std::fs::metadata(local)
            .map_err(|error| format!("deploy_worker `{}`: {error}", local.display()))?;
        if !metadata.is_file() {
            return Err(format!(
                "deploy_worker `{}` is not a regular file",
                local.display()
            ));
        }
        let python = &self.python;

        // Ensure the destination directory exists, then stream the binary.
        let parent = std::path::Path::new(destination)
            .parent()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "/".into());
        let mkdir = format!("mkdir -p {}", shq(&parent));
        self.run("prepare_directory", &mkdir).await?;
        self.run(
            "check_python",
            &format!("{} -I -c {}", shq(python), shq("import sys; print(sys.version); assert sys.version_info >= (3, 7), 'Python 3.7+ required'")),
        )
        .await?;

        // The ssh backend refuses a root of `/`, so the operator is rooted at
        // the destination's parent directory and the file name is the object.
        let destination_path = std::path::Path::new(destination);
        let (root, file_name) = (
            destination_path
                .parent()
                .map(|parent| parent.display().to_string())
                .unwrap_or_else(|| "/public".into()),
            destination_path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("worker deploy destination must end in a file name")?
                .to_string(),
        );
        if root == "/" || file_name.is_empty() {
            return Err("worker deploy destination must be a file inside a directory".into());
        }
        let operator =
            vfs::build_ssh_operator_with_program(&self.alias, &root, python, None, &self.program)
                .map_err(|error| format!("worker deploy channel: {error}"))?;
        let object_path = file_name;
        tracing::info!(target = %self.id, alias = %self.alias, stage = "upload", bytes = metadata.len(), "cluster step started");
        self.progress.upload(
            &format!("正在上传 worker，共 {} 字节", metadata.len()),
            0,
            metadata.len(),
        );
        let mut writer = operator
            .writer(&object_path)
            .await
            .map_err(|error| format!("worker deploy open failed: {error}"))?;
        let file = tokio::fs::File::open(local)
            .await
            .map_err(|error| format!("deploy_worker `{}`: {error}", local.display()))?;
        let mut reader = tokio::io::BufReader::with_capacity(1024 * 1024, file);
        let mut chunk = vec![0u8; 1024 * 1024];
        let mut uploaded = 0u64;
        let mut progress = 0u64;
        loop {
            let read = tokio::io::AsyncReadExt::read(&mut reader, &mut chunk)
                .await
                .map_err(|error| format!("worker deploy read failed: {error}"))?;
            if read == 0 {
                break;
            }
            let buffer = opendal::Buffer::from(chunk[..read].to_vec());
            opendal::Writer::write(&mut writer, buffer)
            .await
            .map_err(|error| {
                tracing::error!(target = %self.id, stage = "upload", uploaded, %error, "cluster step failed");
                format!("worker deploy upload failed after {uploaded} bytes: {error}")
            })?;
            uploaded += read as u64;
            if uploaded / (16 * 1024 * 1024) > progress {
                progress = uploaded / (16 * 1024 * 1024);
                self.progress.upload(
                    &format!("已上传 {uploaded}/{} 字节", metadata.len()),
                    uploaded,
                    metadata.len(),
                );
                tracing::info!(target = %self.id, stage = "upload", uploaded, total = metadata.len(), "cluster upload progress");
            }
        }
        tracing::info!(target = %self.id, stage = "commit_upload", uploaded, "cluster step started");
        self.progress
            .upload("上传完成，正在提交", uploaded, metadata.len());
        self.progress
            .report("commit_upload", "正在提交上传文件（原子替换）");
        opendal::Writer::close(&mut writer)
            .await
            .map_err(|error| {
                tracing::error!(target = %self.id, stage = "commit_upload", %error, "cluster step failed");
                format!("worker deploy commit failed: {error}")
            })?;
        tracing::info!(target = %self.id, stage = "upload", uploaded, "cluster step finished");
        let chmod = format!("chmod +x {}", shq(destination));
        self.run("chmod", &chmod).await?;
        tracing::info!(
            destination,
            bytes = metadata.len(),
            "deployed worker binary"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn connection_output_is_observed_before_command_completes() {
        let dir = tempfile::tempdir().unwrap();
        let release = dir.path().join("release");
        let program = dir.path().join("ssh");
        std::fs::write(&program, format!("#!/bin/bash\nprintf 'connected-live\\n'\nwhile ! test -e {}; do sleep 0.01; done\nexec bash -s\n", shq(release.to_str().unwrap()))).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let release_signal = release.clone();
        let connection =
            ClusterConnection::new("test", "cluster", program.to_str().unwrap(), "python3")
                .with_progress(EnrollProgress::new(move |_, message| {
                    if message.contains("stdout: connected-live") {
                        std::fs::write(&release_signal, b"observed").unwrap();
                    }
                }));
        tokio::time::timeout(std::time::Duration::from_secs(5), connection.authenticate())
            .await
            .unwrap()
            .unwrap();
        assert!(release.exists());
    }

    #[tokio::test]
    async fn deployment_uses_same_custom_transport_for_commands_and_upload() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("ssh");
        std::fs::write(&program, "#!/bin/bash\nlast=\"${!#}\"\nif [ \"$last\" = -s ]; then exec bash -s; fi\nexec bash -c \"$last\"\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let source = dir.path().join("source");
        std::fs::write(&source, b"worker bytes").unwrap();
        let destination = dir.path().join("remote/worker");
        let stages = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let reported = stages.clone();
        let connection =
            ClusterConnection::new("test", "cluster", program.to_str().unwrap(), "python3")
                .with_progress(EnrollProgress::with_events(move |event| {
                    reported.lock().unwrap().push(event.clone());
                }));
        connection.authenticate().await.unwrap();
        connection
            .deploy_worker(source.to_str().unwrap(), destination.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"worker bytes");
        assert_ne!(
            std::fs::metadata(&destination)
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
        assert_eq!(
            connection
                .probe_worker(destination.to_str().unwrap())
                .await
                .unwrap()
                .len(),
            64
        );
        let events = stages.lock().unwrap().clone();
        let upload: Vec<_> = events
            .iter()
            .filter(|event| event.stage == "upload")
            .collect();
        assert_eq!(upload.first().unwrap().uploaded_bytes, Some(0));
        assert_eq!(upload.last().unwrap().uploaded_bytes, Some(12));
        assert!(upload.iter().all(|event| event.total_bytes == Some(12)));
        let mut stages: Vec<_> = events.iter().map(|event| event.stage.as_str()).collect();
        stages.dedup();
        assert_eq!(
            stages,
            [
                "connection",
                "authenticate",
                "prepare_directory",
                "check_python",
                "upload",
                "commit_upload",
                "chmod",
                "probe_worker"
            ]
        );
    }

    #[tokio::test]
    async fn command_failures_preserve_stage_and_remote_output() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("ssh");
        std::fs::write(&program, "#!/bin/sh\nprintf 'cluster banner\\n'\nprintf 'authentication refused\\n' >&2\nexit 255\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let connection =
            ClusterConnection::new("test", "cluster", program.to_str().unwrap(), "python3");
        let error = connection.authenticate().await.unwrap_err();
        assert!(error.contains("authenticate"));
        assert!(error.contains("cluster banner"));
        assert!(error.contains("authentication refused"));
        assert!(error.contains("255"));
    }
    #[tokio::test]
    async fn closed_stdin_preserves_remote_diagnostics_without_retry_or_false_success() {
        for status in [255, 0] {
            let root = tempfile::tempdir().unwrap();
            let program = root.path().join("ssh");
            let attempts = root.path().join("attempts");
            std::fs::write(&program, format!(
                "#!/bin/sh\nprintf 'attempt\\n' >> '{}'\nprintf 'cluster banner\\n'\nprintf 'authentication refused\\n' >&2\nexit {status}\n",
                attempts.display()
            )).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
            // More than a pipe can buffer: SSH exits without consuming any payload.
            let script = "true\n".repeat(65536);
            let target = dag_core::SshTarget::new(program.to_str().unwrap(), "cluster");
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                target.run_script_once(&script),
            )
            .await
            .unwrap();
            if status == 255 {
                let output = result.unwrap();
                assert_eq!(output.status.code(), Some(255));
                assert_eq!(output.stdout, b"cluster banner\n");
                assert_eq!(output.stderr, b"authentication refused\n");
            } else {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("cannot pipe ssh stdin"), "{error}");
                assert!(error.contains("cluster banner"), "{error}");
                assert!(error.contains("authentication refused"), "{error}");
            }
            assert_eq!(std::fs::read_to_string(attempts).unwrap(), "attempt\n");
        }
    }
}
