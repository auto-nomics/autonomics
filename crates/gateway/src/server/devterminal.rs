//! PTY-backed shell attached to an existing development container.

use container_runtime::PodmanConfig;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    sync::{
        Arc, Mutex,
        mpsc::{SyncSender, sync_channel},
    },
};

const OUTPUT_LIMIT: usize = 1024 * 1024;

#[derive(Default)]
struct Output {
    bytes: VecDeque<u8>,
    start: u64,
    closed: bool,
}

pub(super) struct Terminal {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    input: SyncSender<Vec<u8>>,
    output: Arc<Mutex<Output>>,
}

impl Terminal {
    pub fn start(
        config: &PodmanConfig,
        container: &str,
        rows: u16,
        cols: u16,
        remote_env: &BTreeMap<String, String>,
        remote_user: Option<&str>,
    ) -> Result<Self, String> {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        let mut podman = config.blocking_command();
        podman.args([
            "exec",
            "--interactive",
            "--tty",
            "--workdir=/work",
            "--env=TERM=xterm-256color",
        ]);
        podman.args(remote_user.map(|user| format!("--user={user}")));
        podman.args(
            remote_env
                .iter()
                .map(|(key, value)| format!("--env={key}={value}")),
        );
        podman.args([container, "/bin/sh", "-i"]);
        let mut command = CommandBuilder::new(podman.get_program());
        command.args(podman.get_args());
        for (key, value) in podman.get_envs() {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
        if let Some(cwd) = podman.get_current_dir() {
            command.cwd(cwd);
        }
        let child = pty
            .slave
            .spawn_command(command)
            .map_err(|e| e.to_string())?;
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().map_err(|e| e.to_string())?;
        let mut writer = pty.master.take_writer().map_err(|e| e.to_string())?;
        let output = Arc::new(Mutex::new(Output::default()));
        let read_output = output.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut output = read_output.lock().unwrap_or_else(|e| e.into_inner());
                output.bytes.extend(&buf[..n]);
                let excess = output.bytes.len().saturating_sub(OUTPUT_LIMIT);
                if excess > 0 {
                    output.bytes.drain(..excess);
                    output.start += excess as u64;
                }
            }
            read_output.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
        });
        let (input, receiver) = sync_channel::<Vec<u8>>(64);
        std::thread::spawn(move || {
            while let Ok(bytes) = receiver.recv() {
                if writer.write_all(&bytes).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            master: pty.master,
            child,
            input,
            output,
        })
    }

    pub fn exited(&mut self) -> Result<bool, String> {
        self.child
            .try_wait()
            .map(|status| status.is_some())
            .map_err(|e| e.to_string())
    }

    pub fn write(&self, input: String) -> Result<(), String> {
        if input.len() > 65536 {
            return Err("终端输入过长".into());
        }
        self.input
            .try_send(input.into_bytes())
            .map_err(|e| format!("终端输入不可用：{e}"))
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        if rows == 0 || cols == 0 || rows > 1000 || cols > 1000 {
            return Err("终端尺寸无效".into());
        }
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())
    }

    pub fn read(&self, offset: u64) -> (Vec<u8>, u64, bool) {
        let output = self.output.lock().unwrap_or_else(|e| e.into_inner());
        let end = output.start + output.bytes.len() as u64;
        let cursor = offset.clamp(output.start, end);
        (
            output
                .bytes
                .iter()
                .skip((cursor - output.start) as usize)
                .copied()
                .collect(),
            end,
            offset < output.start,
        )
    }

    pub fn output_closed(&self) -> bool {
        self.output.lock().unwrap_or_else(|e| e.into_inner()).closed
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
