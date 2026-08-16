//! Clipboard copy backend for message copying.
//!
//! The backend order depends on where the user's clipboard actually lives. In
//! an SSH session, the native clipboard belongs to the user's local terminal,
//! so terminal-mediated copy is used. In a local session, the native clipboard
//! is tried first. Linux X11 and some Wayland compositors require the writer
//! to retain clipboard ownership, so callers must keep the returned lease
//! alive for the lifetime of the TUI.

use base64::Engine;
use std::io::Write;

const OSC52_MAX_RAW_BYTES: usize = 100_000;

/// Copy text to the clipboard and return an owner lease when one is required.
pub fn copy_to_clipboard(text: &str) -> Result<Option<ClipboardLease>, String> {
    copy_to_clipboard_with(
        text,
        CopyEnvironment {
            ssh_session: is_ssh_session(),
            wsl_session: is_wsl_session(),
            tmux_session: is_tmux_session(),
        },
        tmux_clipboard_copy,
        osc52_copy,
        arboard_copy,
        wsl_clipboard_copy,
    )
}

pub struct ClipboardLease {
    #[cfg(target_os = "linux")]
    _clipboard: Option<arboard::Clipboard>,
}

impl ClipboardLease {
    #[cfg(target_os = "linux")]
    fn native_linux(clipboard: arboard::Clipboard) -> Self {
        Self {
            _clipboard: Some(clipboard),
        }
    }

    #[cfg(test)]
    pub(crate) fn test() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            _clipboard: None,
        }
    }
}

#[derive(Clone, Copy)]
struct CopyEnvironment {
    ssh_session: bool,
    wsl_session: bool,
    tmux_session: bool,
}

fn copy_to_clipboard_with(
    text: &str,
    environment: CopyEnvironment,
    tmux_copy_fn: impl Fn(&str) -> Result<(), String>,
    osc52_copy_fn: impl Fn(&str) -> Result<(), String>,
    arboard_copy_fn: impl Fn(&str) -> Result<Option<ClipboardLease>, String>,
    wsl_copy_fn: impl Fn(&str) -> Result<(), String>,
) -> Result<Option<ClipboardLease>, String> {
    if environment.ssh_session {
        return terminal_clipboard_copy_with(
            text,
            environment.tmux_session,
            &tmux_copy_fn,
            &osc52_copy_fn,
        )
        .map(|()| None)
        .map_err(|terminal_error| {
            let path = if environment.tmux_session {
                "terminal clipboard copy failed over SSH"
            } else {
                "OSC 52 clipboard copy failed over SSH"
            };
            format!("{path}: {terminal_error}")
        });
    }

    match arboard_copy_fn(text) {
        Ok(lease) => Ok(lease),
        Err(native_error) => {
            if environment.wsl_session {
                tracing::warn!(
                    "native clipboard copy failed: {native_error}; trying Windows clipboard"
                );
                if let Err(wsl_error) = wsl_copy_fn(text) {
                    tracing::warn!(
                        "Windows clipboard copy failed: {wsl_error}; trying terminal clipboard"
                    );
                    return terminal_clipboard_copy_with(
                        text,
                        environment.tmux_session,
                        &tmux_copy_fn,
                        &osc52_copy_fn,
                    )
                    .map(|()| None)
                    .map_err(|terminal_error| {
                        format!(
                            "native clipboard: {native_error}; Windows fallback: {wsl_error}; terminal fallback: {terminal_error}"
                        )
                    });
                }
                return Ok(None);
            }

            tracing::warn!(
                "native clipboard copy failed: {native_error}; trying terminal clipboard"
            );
            terminal_clipboard_copy_with(
                text,
                environment.tmux_session,
                &tmux_copy_fn,
                &osc52_copy_fn,
            )
            .map(|()| None)
            .map_err(|terminal_error| {
                format!("native clipboard: {native_error}; terminal fallback: {terminal_error}")
            })
        }
    }
}

fn terminal_clipboard_copy_with(
    text: &str,
    tmux_session: bool,
    tmux_copy_fn: &impl Fn(&str) -> Result<(), String>,
    osc52_copy_fn: &impl Fn(&str) -> Result<(), String>,
) -> Result<(), String> {
    if tmux_session {
        return match tmux_copy_fn(text) {
            Ok(()) => Ok(()),
            Err(tmux_error) => osc52_copy_fn(text).map_err(|osc52_error| {
                format!("tmux clipboard: {tmux_error}; OSC 52 fallback: {osc52_error}")
            }),
        };
    }

    osc52_copy_fn(text)
}

fn is_ssh_session() -> bool {
    std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some()
}

fn is_tmux_session() -> bool {
    std::env::var_os("TMUX").is_some() || std::env::var_os("TMUX_PANE").is_some()
}

#[cfg(target_os = "linux")]
fn is_wsl_session() -> bool {
    if let Ok(version) = std::fs::read_to_string("/proc/version") {
        let version = version.to_lowercase();
        if version.contains("microsoft") || version.contains("wsl") {
            return true;
        }
    }

    std::env::var_os("WSL_DISTRO_NAME").is_some() || std::env::var_os("WSL_INTEROP").is_some()
}

#[cfg(not(target_os = "linux"))]
fn is_wsl_session() -> bool {
    false
}

#[cfg(all(not(target_os = "android"), not(target_os = "linux")))]
fn arboard_copy(text: &str) -> Result<Option<ClipboardLease>, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("failed to set clipboard text: {e}"))?;
    Ok(None)
}

#[cfg(target_os = "linux")]
fn arboard_copy(text: &str) -> Result<Option<ClipboardLease>, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("failed to set clipboard text: {e}"))?;
    Ok(Some(ClipboardLease::native_linux(clipboard)))
}

#[cfg(target_os = "android")]
fn arboard_copy(_text: &str) -> Result<Option<ClipboardLease>, String> {
    Err("native clipboard unavailable on Android".to_string())
}

#[cfg(target_os = "linux")]
fn wsl_clipboard_copy(text: &str) -> Result<(), String> {
    let mut child = std::process::Command::new("powershell.exe")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .args([
            "-NoProfile",
            "-Command",
            "[Console]::InputEncoding = [System.Text.Encoding]::UTF8; $ErrorActionPreference = 'Stop'; $text = [Console]::In.ReadToEnd(); Set-Clipboard -Value $text",
        ])
        .spawn()
        .map_err(|e| format!("failed to spawn powershell.exe: {e}"))?;

    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("failed to open powershell.exe stdin".to_string());
    };
    if let Err(error) = stdin.write_all(text.as_bytes()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("failed to write to powershell.exe: {error}"));
    }
    drop(stdin);

    let output = child
        .wait_with_output()
        .map_err(|e| format!("failed to wait for powershell.exe: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error(
            "powershell.exe",
            output.status,
            &output.stderr,
        ))
    }
}

#[cfg(not(target_os = "linux"))]
fn wsl_clipboard_copy(_text: &str) -> Result<(), String> {
    Err("Windows clipboard fallback unavailable on this platform".to_string())
}

fn command_error(command: &str, status: std::process::ExitStatus, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    if stderr.is_empty() {
        format!("{command} exited with status {status}")
    } else {
        format!("{command} failed: {stderr}")
    }
}

fn tmux_clipboard_copy(text: &str) -> Result<(), String> {
    tmux_clipboard_copy_ready(
        || tmux_command_output(["show-options", "-gv", "set-clipboard"]),
        || tmux_command_output(["info"]),
    )?;

    let mut child = std::process::Command::new("tmux")
        .args(["load-buffer", "-w", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn tmux: {e}"))?;

    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("failed to open tmux stdin".to_string());
    };
    if let Err(error) = stdin.write_all(text.as_bytes()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("failed to write to tmux: {error}"));
    }
    drop(stdin);

    let output = child
        .wait_with_output()
        .map_err(|e| format!("failed to wait for tmux: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error("tmux", output.status, &output.stderr))
    }
}

fn tmux_clipboard_copy_ready(
    set_clipboard_fn: impl FnOnce() -> Result<String, String>,
    tmux_info_fn: impl FnOnce() -> Result<String, String>,
) -> Result<(), String> {
    let set_clipboard = set_clipboard_fn()?;
    if set_clipboard.trim() == "off" {
        return Err("tmux clipboard forwarding is disabled".to_string());
    }

    let tmux_info = tmux_info_fn()?;
    if tmux_info.lines().any(|line| line.contains("Ms: [missing]")) {
        return Err("tmux clipboard forwarding is unavailable: missing Ms capability".to_string());
    }

    Ok(())
}

fn tmux_command_output<const N: usize>(args: [&str; N]) -> Result<String, String> {
    let output = std::process::Command::new("tmux")
        .args(args)
        .output()
        .map_err(|e| format!("failed to spawn tmux: {e}"))?;
    if output.status.success() {
        String::from_utf8(output.stdout).map_err(|e| format!("tmux output was not UTF-8: {e}"))
    } else {
        Err(command_error("tmux", output.status, &output.stderr))
    }
}

fn osc52_copy(text: &str) -> Result<(), String> {
    let sequence = osc52_sequence(text, std::env::var_os("TMUX").is_some())?;

    #[cfg(unix)]
    {
        if let Ok(tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty")
            && write_osc52_to_writer(tty, &sequence).is_ok()
        {
            return Ok(());
        }
    }

    write_osc52_to_writer(std::io::stdout().lock(), &sequence)
}

fn write_osc52_to_writer(mut writer: impl Write, sequence: &str) -> Result<(), String> {
    writer
        .write_all(sequence.as_bytes())
        .map_err(|e| format!("failed to write OSC 52: {e}"))?;
    writer
        .flush()
        .map_err(|e| format!("failed to flush OSC 52: {e}"))
}

fn osc52_sequence(text: &str, tmux: bool) -> Result<String, String> {
    if text.len() > OSC52_MAX_RAW_BYTES {
        return Err(format!(
            "OSC 52 payload too large ({} bytes; max {OSC52_MAX_RAW_BYTES})",
            text.len()
        ));
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    if tmux {
        Ok(format!("\x1bPtmux;\x1b\x1b]52;c;{encoded}\x07\x1b\\"))
    } else {
        Ok(format!("\x1b]52;c;{encoded}\x07"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn local_environment() -> CopyEnvironment {
        CopyEnvironment {
            ssh_session: false,
            wsl_session: false,
            tmux_session: false,
        }
    }

    fn remote_tmux_environment() -> CopyEnvironment {
        CopyEnvironment {
            ssh_session: true,
            wsl_session: false,
            tmux_session: true,
        }
    }

    #[test]
    fn ssh_inside_tmux_prefers_tmux_clipboard() {
        let tmux_calls = Cell::new(0);
        let osc52_calls = Cell::new(0);
        let native_calls = Cell::new(0);
        let result = copy_to_clipboard_with(
            "hello",
            remote_tmux_environment(),
            |_| {
                tmux_calls.set(tmux_calls.get() + 1);
                Ok(())
            },
            |_| {
                osc52_calls.set(osc52_calls.get() + 1);
                panic!("OSC 52 must not run after tmux succeeds");
            },
            |_| {
                native_calls.set(native_calls.get() + 1);
                panic!("native clipboard must not run over SSH");
            },
            |_| panic!("Windows fallback must not run over SSH"),
        );

        assert!(result.is_ok());
        assert_eq!(tmux_calls.get(), 1);
        assert_eq!(osc52_calls.get(), 0);
        assert_eq!(native_calls.get(), 0);
    }

    #[test]
    fn ssh_inside_tmux_falls_back_to_osc52_when_tmux_fails() {
        let tmux_calls = Cell::new(0);
        let osc52_calls = Cell::new(0);
        let result = copy_to_clipboard_with(
            "hello",
            remote_tmux_environment(),
            |_| {
                tmux_calls.set(tmux_calls.get() + 1);
                Err("forwarding disabled".to_string())
            },
            |_| {
                osc52_calls.set(osc52_calls.get() + 1);
                Ok(())
            },
            |_| panic!("native clipboard must not run over SSH"),
            |_| panic!("Windows fallback must not run over SSH"),
        );

        assert!(result.is_ok());
        assert_eq!(tmux_calls.get(), 1);
        assert_eq!(osc52_calls.get(), 1);
    }

    #[test]
    fn local_uses_native_clipboard_first() {
        let osc52_calls = Cell::new(0);
        let result = copy_to_clipboard_with(
            "hello",
            local_environment(),
            |_| panic!("tmux must not run outside tmux"),
            |_| {
                osc52_calls.set(osc52_calls.get() + 1);
                panic!("OSC 52 must not run after native succeeds");
            },
            |_| Ok(Some(ClipboardLease::test())),
            |_| panic!("Windows fallback must not run outside WSL"),
        );

        assert!(matches!(result, Ok(Some(_))));
        assert_eq!(osc52_calls.get(), 0);
    }

    #[test]
    fn local_falls_back_to_osc52_when_native_fails() {
        let osc52_calls = Cell::new(0);
        let result = copy_to_clipboard_with(
            "hello",
            local_environment(),
            |_| panic!("tmux must not run outside tmux"),
            |_| {
                osc52_calls.set(osc52_calls.get() + 1);
                Ok(())
            },
            |_| Err("no display".to_string()),
            |_| panic!("Windows fallback must not run outside WSL"),
        );

        assert!(result.is_ok());
        assert_eq!(osc52_calls.get(), 1);
    }

    #[test]
    fn tmux_readiness_detects_disabled_forwarding() {
        let result = tmux_clipboard_copy_ready(
            || Ok("off\n".to_string()),
            || panic!("tmux info should not be read when forwarding is disabled"),
        );

        assert_eq!(
            result,
            Err("tmux clipboard forwarding is disabled".to_string())
        );
    }

    #[test]
    fn tmux_readiness_detects_missing_ms_capability() {
        let result = tmux_clipboard_copy_ready(
            || Ok("external\n".to_string()),
            || Ok("193: Ms: [missing]\n".to_string()),
        );

        assert_eq!(
            result,
            Err("tmux clipboard forwarding is unavailable: missing Ms capability".to_string())
        );
    }

    #[test]
    fn osc52_sequence_round_trips_and_wraps_tmux() {
        let sequence = osc52_sequence("hello", true).unwrap();
        assert_eq!(sequence, "\x1bPtmux;\x1b\x1b]52;c;aGVsbG8=\x07\x1b\\");
    }
}
