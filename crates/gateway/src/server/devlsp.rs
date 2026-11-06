//! Language servers run inside an already connected development container.
use container_runtime::PodmanConfig;
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin},
    time::timeout,
};

const MAX_MESSAGE: usize = 8 * 1024 * 1024;
const MAX_QUEUE: usize = 8 * 1024 * 1024;

#[derive(Default)]
struct Output {
    next: u64,
    bytes: usize,
    messages: VecDeque<(u64, usize, Value)>,
    closed: bool,
}

pub(super) struct LanguageServer {
    child: Child,
    stdin: ChildStdin,
    output: Arc<Mutex<Output>>,
    last_access: Instant,
}

fn server_command(language: &str) -> Option<(&'static str, &'static str)> {
    match language {
        "python" => Some((
            "[ -x /work/.venv/bin/pylsp ] || [ -x /work/.venv/bin/pyright-langserver ] || [ -x /work/node_modules/.bin/pyright-langserver ] || command -v pylsp >/dev/null || command -v pyright-langserver >/dev/null",
            "if [ -x /work/.venv/bin/pylsp ]; then exec /work/.venv/bin/pylsp; elif [ -x /work/.venv/bin/pyright-langserver ]; then exec /work/.venv/bin/pyright-langserver --stdio; elif [ -x /work/node_modules/.bin/pyright-langserver ]; then exec /work/node_modules/.bin/pyright-langserver --stdio; elif command -v pylsp >/dev/null; then exec pylsp; else exec pyright-langserver --stdio; fi",
        )),
        "r" => Some((
            "command -v Rscript >/dev/null && Rscript -e 'quit(status=ifelse(requireNamespace(\"languageserver\", quietly=TRUE),0,1))'",
            "exec Rscript -e 'languageserver::run()'",
        )),
        "rust" => Some(("command -v rust-analyzer >/dev/null", "exec rust-analyzer")),
        "typescript" | "javascript" => Some((
            "[ -x /work/node_modules/.bin/typescript-language-server ] || command -v typescript-language-server >/dev/null",
            "if [ -x /work/node_modules/.bin/typescript-language-server ]; then exec /work/node_modules/.bin/typescript-language-server --stdio; else exec typescript-language-server --stdio; fi",
        )),
        "c" | "cpp" => Some(("command -v clangd >/dev/null", "exec clangd --log=error")),
        "go" => Some((
            "command -v gopls >/dev/null || [ -x \"$HOME/go/bin/gopls\" ]",
            "if command -v gopls >/dev/null; then exec gopls serve; else exec \"$HOME/go/bin/gopls\" serve; fi",
        )),
        _ => None,
    }
}

impl LanguageServer {
    pub async fn start_debug(
        config: &PodmanConfig,
        container: &str,
        language: &str,
        remote_env: &BTreeMap<String, String>,
        remote_user: Option<&str>,
    ) -> Result<Self, String> {
        if language != "python" {
            return Err("不支持的调试语言".into());
        }
        let probe = "if [ -x /work/.venv/bin/python ] && /work/.venv/bin/python -c 'import debugpy' >/dev/null 2>&1; then exit 0; fi; command -v python3 >/dev/null && python3 -c 'import debugpy' >/dev/null 2>&1";
        let status = timeout(
            Duration::from_secs(15),
            config
                .async_command()
                .arg("exec")
                .args(remote_user.map(|user| format!("--user={user}")))
                .args(
                    remote_env
                        .iter()
                        .map(|(key, value)| format!("--env={key}={value}")),
                )
                .args([container, "/bin/sh", "-c", probe])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status(),
        )
        .await
        .map_err(|_| "调试器检查超时".to_string())?
        .map_err(|error| error.to_string())?;
        if !status.success() {
            return Err("容器内未安装 Python debugpy 调试器".into());
        }
        let command = "if [ -x /work/.venv/bin/python ] && /work/.venv/bin/python -c 'import debugpy' >/dev/null 2>&1; then exec /work/.venv/bin/python -m debugpy.adapter; else exec python3 -m debugpy.adapter; fi";
        Self::launch(config, container, command, remote_env, remote_user)
    }

    pub async fn start(
        config: &PodmanConfig,
        container: &str,
        language: &str,
        remote_env: &BTreeMap<String, String>,
        remote_user: Option<&str>,
    ) -> Result<Self, String> {
        let (probe, command) =
            server_command(language).ok_or_else(|| "不支持的语言服务器".to_string())?;
        let status = timeout(
            Duration::from_secs(15),
            config
                .async_command()
                .arg("exec")
                .args(remote_user.map(|user| format!("--user={user}")))
                .args(
                    remote_env
                        .iter()
                        .map(|(key, value)| format!("--env={key}={value}")),
                )
                .args([container, "/bin/sh", "-c", probe])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status(),
        )
        .await
        .map_err(|_| "语言服务器检查超时".to_string())?
        .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err(format!("容器内未安装 {language} 语言服务器"));
        }
        Self::launch(config, container, command, remote_env, remote_user)
    }

    pub(super) fn launch(
        config: &PodmanConfig,
        container: &str,
        command: &str,
        remote_env: &BTreeMap<String, String>,
        remote_user: Option<&str>,
    ) -> Result<Self, String> {
        let mut child = config
            .async_command()
            .args(["exec", "--interactive", "--workdir=/work"])
            .args(remote_user.map(|user| format!("--user={user}")))
            .args(
                remote_env
                    .iter()
                    .map(|(key, value)| format!("--env={key}={value}")),
            )
            .args([container, "/bin/sh", "-c", command])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| e.to_string())?;
        let stdin = child.stdin.take().ok_or("无法连接语言服务器输入")?;
        let stdout = child.stdout.take().ok_or("无法连接语言服务器输出")?;
        let output = Arc::new(Mutex::new(Output::default()));
        let sink = output.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut length = None;
                let mut header_bytes = 0;
                loop {
                    let mut line = String::new();
                    let n = match (&mut reader).take(8193).read_line(&mut line).await {
                        Ok(n) if n > 0 => n,
                        _ => {
                            sink.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
                            return;
                        }
                    };
                    header_bytes += n;
                    if header_bytes > 8192 {
                        sink.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
                        return;
                    }
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                    if let Some((key, value)) = line.trim().split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            length = value.trim().parse::<usize>().ok();
                        }
                    }
                }
                let Some(length) = length.filter(|n| *n <= MAX_MESSAGE) else {
                    break;
                };
                let mut bytes = vec![0; length];
                if reader.read_exact(&mut bytes).await.is_err() {
                    break;
                }
                let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                    break;
                };
                let mut queue = sink.lock().unwrap_or_else(|e| e.into_inner());
                let seq = queue.next;
                queue.next += 1;
                queue.bytes += length;
                queue.messages.push_back((seq, length, value));
                while queue.bytes > MAX_QUEUE || queue.messages.len() > 1024 {
                    if let Some((_, size, _)) = queue.messages.pop_front() {
                        queue.bytes -= size;
                    } else {
                        break;
                    }
                }
            }
            sink.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        });
        Ok(Self {
            child,
            stdin,
            output,
            last_access: Instant::now(),
        })
    }

    pub async fn send(&mut self, value: Value) -> Result<(), String> {
        self.last_access = Instant::now();
        let bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_MESSAGE {
            return Err("语言服务消息过大".into());
        }
        timeout(Duration::from_secs(10), async {
            self.stdin
                .write_all(format!("Content-Length: {}\r\n\r\n", bytes.len()).as_bytes())
                .await?;
            self.stdin.write_all(&bytes).await?;
            self.stdin.flush().await
        })
        .await
        .map_err(|_| "语言服务器输入超时".to_string())?
        .map_err(|e| e.to_string())
    }

    pub fn poll(&mut self, after: u64) -> Result<(Vec<Value>, u64, bool, bool), String> {
        self.last_access = Instant::now();
        let exited = self.child.try_wait().map_err(|e| e.to_string())?.is_some();
        let queue = self.output.lock().unwrap_or_else(|e| e.into_inner());
        let start = queue
            .messages
            .front()
            .map_or(queue.next, |(seq, _, _)| *seq);
        let truncated = after < start;
        let messages = queue
            .messages
            .iter()
            .filter(|(seq, _, _)| *seq >= after)
            .map(|(_, _, value)| value.clone())
            .collect();
        Ok((messages, queue.next, truncated, exited && queue.closed))
    }

    pub fn expired(&mut self) -> bool {
        self.last_access.elapsed() > Duration::from_secs(120)
            || self.child.try_wait().ok().flatten().is_some()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn protocol_round_trip_and_unsupported_language() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("podman-mock");
        std::fs::write(
            &program,
            "#!/bin/sh\ncase \"$*\" in *--interactive*) exec cat;; *) exit 0;; esac\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = PodmanConfig::default();
        config.program = program.to_string_lossy().into_owned();
        config.storage_root = None;
        config.run_root = None;
        config.storage_driver = None;
        assert!(
            LanguageServer::start(&config, "test", "unknown", &BTreeMap::new(), None)
                .await
                .is_err()
        );
        let mut server = LanguageServer::start(&config, "test", "python", &BTreeMap::new(), None)
            .await
            .expect("start mock server");
        let message = serde_json::json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{"name":"科学"}});
        server.send(message.clone()).await.expect("send message");
        let received = timeout(Duration::from_secs(5), async {
            loop {
                let (messages, offset, truncated, exited) = server.poll(0).unwrap();
                if !messages.is_empty() {
                    break (messages, offset, truncated, exited);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("server response");
        assert_eq!(received.0, vec![message]);
        assert_eq!(received.1, 1);
        assert!(!received.2 && !received.3);
        assert!(server.poll(1).unwrap().0.is_empty());
    }
}
