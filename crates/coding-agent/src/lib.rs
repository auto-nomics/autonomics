//! A host-owned interface for external coding agents.
//!
//! This crate deliberately knows nothing about plugin repositories or RSI
//! policy. Callers construct validated tasks, choose an explicit argv, and
//! receive bounded evidence suitable for durable reports.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

pub const MAX_PROMPT_BYTES: usize = 64 * 1024;
pub const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("coding agent operation failed: {0}")]
    CodingAgent(String),

    #[error("coding agent `{agent}` exceeded its {timeout_secs}s timeout")]
    Timeout { agent: String, timeout_secs: u64 },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The external coding agents supported by the default adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingAgentKind {
    Codex,
    ClaudeCode,
    PiAgent,
    Custom(String),
}

impl CodingAgentKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
            Self::PiAgent => "pi-agent",
            Self::Custom(name) => name,
        }
    }
}

/// Daemon-selected invocation policy for one external coding agent.
///
/// Arguments are explicit argv values. The host never builds a shell command
/// and callers should never let untrusted input mutate this configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodingAgentConfig {
    pub kind: CodingAgentKind,
    pub executable: String,
    pub arguments: Vec<String>,
    #[serde(default = "default_environment_allowlist")]
    pub environment_allowlist: BTreeSet<String>,
    pub timeout_secs: u64,
}

impl CodingAgentConfig {
    pub fn new(
        kind: CodingAgentKind,
        executable: impl Into<String>,
        arguments: Vec<String>,
    ) -> Result<Self> {
        let config = Self {
            kind,
            executable: executable.into(),
            arguments,
            environment_allowlist: default_environment_allowlist(),
            timeout_secs: 900,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn codex() -> Result<Self> {
        Self::new(
            CodingAgentKind::Codex,
            "codex",
            vec![
                "exec".into(),
                "--sandbox".into(),
                "workspace-write".into(),
                "-".into(),
            ],
        )
    }

    pub fn claude_code() -> Result<Self> {
        Self::new(
            CodingAgentKind::ClaudeCode,
            "claude",
            vec![
                "--print".into(),
                "--permission-mode".into(),
                "acceptEdits".into(),
                "--output-format".into(),
                "text".into(),
            ],
        )
    }

    pub fn pi_agent() -> Result<Self> {
        Self::new(CodingAgentKind::PiAgent, "pi-agent", vec!["--stdin".into()])
    }

    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    pub fn with_environment_allowlist<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.environment_allowlist = names.into_iter().map(Into::into).collect();
        self
    }

    pub fn validate(&self) -> Result<()> {
        if self.executable.trim().is_empty() {
            return Err(Error::CodingAgent("executable is required".into()));
        }
        if self.timeout_secs == 0 {
            return Err(Error::CodingAgent("timeout must be nonzero".into()));
        }
        if self.arguments.iter().any(|arg| arg.contains('\0')) {
            return Err(Error::CodingAgent("arguments cannot contain NUL".into()));
        }
        Ok(())
    }
}

fn default_environment_allowlist() -> BTreeSet<String> {
    ["PATH", "HOME", "USER", "LANG", "TERM"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// A development request sent to an external coding agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodingTask {
    pub instructions: String,
    pub request_context: String,
}

impl CodingTask {
    pub fn new(instructions: impl Into<String>) -> Self {
        Self {
            instructions: instructions.into(),
            request_context: String::new(),
        }
    }

    pub fn with_request_context(mut self, context: impl Into<String>) -> Self {
        self.request_context = context.into();
        self
    }

    pub fn validate(&self) -> Result<()> {
        if self.instructions.trim().is_empty() {
            return Err(Error::CodingAgent(
                "coding instructions are required".into(),
            ));
        }
        if self.instructions.len() > MAX_PROMPT_BYTES {
            return Err(Error::CodingAgent(format!(
                "coding instructions exceed {MAX_PROMPT_BYTES} bytes"
            )));
        }
        Ok(())
    }
}

/// A process invocation prepared by the host for an external agent.
#[derive(Debug, Clone)]
pub struct CodingCommand {
    pub executable: String,
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub prompt: String,
    pub timeout_secs: u64,
}

/// Raw output returned by an injectable process backend.
#[derive(Debug, Clone)]
pub struct CodingProcessOutput {
    pub exit_code: Option<i32>,
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub duration_ms: u128,
}

pub trait CodingProcess: Send + Sync {
    fn execute(&self, command: CodingCommand) -> Result<CodingProcessOutput>;
}

/// Production process backend with argv isolation and a hard timeout.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemCodingProcess;

impl CodingProcess for SystemCodingProcess {
    fn execute(&self, command: CodingCommand) -> Result<CodingProcessOutput> {
        let started = Instant::now();
        let mut process = Command::new(&command.executable)
            .args(&command.arguments)
            .current_dir(&command.working_directory)
            .env_clear()
            .envs(&command.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| {
                Error::CodingAgent(format!("cannot launch {}: {source}", command.executable))
            })?;

        let stdin = process.stdin.take();
        let prompt = command.prompt.clone();
        let writer = thread::spawn(move || {
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(prompt.as_bytes());
            }
        });

        let deadline = Duration::from_secs(command.timeout_secs);
        let mut timed_out = false;
        loop {
            match process.try_wait()? {
                Some(_) => break,
                None if started.elapsed() >= deadline => {
                    timed_out = true;
                    let _ = process.kill();
                    let _ = process.wait();
                    break;
                }
                None => thread::sleep(Duration::from_millis(50)),
            }
        }

        if timed_out {
            let _ = writer.join();
            return Err(Error::Timeout {
                agent: command.executable,
                timeout_secs: command.timeout_secs,
            });
        }

        let output = process.wait_with_output()?;
        let _ = writer.join();
        Ok(CodingProcessOutput {
            exit_code: output.status.code(),
            success: output.status.success(),
            stdout: output.stdout,
            stderr: output.stderr,
            duration_ms: started.elapsed().as_millis(),
        })
    }
}

/// What one agent execution produced, before caller-specific validation.
#[derive(Debug, Clone)]
pub struct CodingAgentExecution {
    pub argv: Vec<String>,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u128,
}

/// The host-owned interface implemented by each external coding agent.
pub trait CodingAgent: Send + Sync {
    fn kind(&self) -> &CodingAgentKind;
    fn execute(&self, task: &CodingTask, working_directory: &Path) -> Result<CodingAgentExecution>;
}

#[derive(Debug, Clone)]
pub struct CodingAgentRunner<P = SystemCodingProcess> {
    config: CodingAgentConfig,
    process: P,
}

impl CodingAgentRunner<SystemCodingProcess> {
    pub fn new(config: CodingAgentConfig) -> Self {
        Self {
            config,
            process: SystemCodingProcess,
        }
    }
}

impl<P: CodingProcess> CodingAgentRunner<P> {
    pub fn with_process(config: CodingAgentConfig, process: P) -> Self {
        Self { config, process }
    }
}

impl<P: CodingProcess> CodingAgent for CodingAgentRunner<P> {
    fn kind(&self) -> &CodingAgentKind {
        &self.config.kind
    }

    fn execute(&self, task: &CodingTask, working_directory: &Path) -> Result<CodingAgentExecution> {
        task.validate()?;
        self.config.validate()?;
        let environment = filtered_environment(&self.config.environment_allowlist);
        let command = CodingCommand {
            executable: self.config.executable.clone(),
            arguments: self.config.arguments.clone(),
            working_directory: working_directory.to_path_buf(),
            environment,
            prompt: render_task_prompt(task),
            timeout_secs: self.config.timeout_secs,
        };
        let argv = std::iter::once(command.executable.clone())
            .chain(command.arguments.iter().cloned())
            .collect();
        let output = self.process.execute(command)?;
        Ok(CodingAgentExecution {
            argv,
            exit_code: output.exit_code,
            success: output.success,
            stdout: bounded_transcript(output.stdout),
            stderr: bounded_transcript(output.stderr),
            duration_ms: output.duration_ms,
        })
    }
}

fn filtered_environment(allowlist: &BTreeSet<String>) -> BTreeMap<String, String> {
    env::vars()
        .filter(|(name, _)| allowlist.contains(name))
        .collect()
}

pub fn render_task_prompt(task: &CodingTask) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "You are an external coding agent. Work only in the current directory.\n\
         Follow host instructions exactly.\n\n",
    );
    if !task.request_context.trim().is_empty() {
        prompt.push_str("Request context:\n");
        prompt.push_str(task.request_context.trim());
        prompt.push_str("\n\n");
    }
    prompt.push_str("Task:\n");
    prompt.push_str(task.instructions.trim());
    prompt.push('\n');
    prompt
}

pub fn bounded_transcript(bytes: Vec<u8>) -> String {
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text.len() > MAX_TRANSCRIPT_BYTES {
        let mut boundary = MAX_TRANSCRIPT_BYTES;
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        text.truncate(boundary);
        text.push_str("\n[transcript truncated]");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_prompt_includes_context_and_instructions() {
        let prompt = render_task_prompt(
            &CodingTask::new(" implement request ")
                .with_request_context(" feedback: keep scope small "),
        );

        assert!(prompt.contains("feedback: keep scope small"));
        assert!(prompt.contains("implement request"));
        assert!(!prompt.contains(" implement request \n"));
    }

    #[test]
    fn transcript_is_bounded_on_utf8_boundary() {
        let transcript = bounded_transcript([0xe4, 0xbd, 0xa0].repeat(30_000));

        assert!(transcript.len() <= MAX_TRANSCRIPT_BYTES + 22);
        assert!(transcript.ends_with("[transcript truncated]"));
    }
}
