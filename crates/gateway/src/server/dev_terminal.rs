//! WebSocket terminal for plugin development sessions.
//!
//! One connection = one `podman exec` shell in the plugin's dev
//! container. Protocol (deliberately minimal, xterm.js-friendly):
//!
//! - client → server: **binary** frames are raw stdin bytes; **text**
//!   frames are control JSON (`{"type":"resize","cols":..,"rows":..}`
//!   is acknowledged with a notice — the podman CLI cannot resize a
//!   live exec, so the frontend offers a fresh shell instead).
//! - server → client: **binary** frames are the merged pty output
//!   (`--tty` folds stderr in); **text** frames are events
//!   (`{"type":"notice",...}`, `{"type":"exit","code":n}` when the
//!   shell exits — the pane then shows a reconnect button; no
//!   auto-respawn).
//!
//! Backpressure: the ws→stdin direction goes through a bounded mpsc so
//! a blocked pty cannot wedge the socket; dropping the socket drops the
//! stdin channel, whose writer task sends EOF, so the shell exits
//! (kill-on-drop on the exec child is the hard backstop).

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};



use super::plugins;
use super::state::GatewayState;

#[derive(serde::Deserialize)]
pub(crate) struct TerminalQuery {
    cols: Option<u16>,
    rows: Option<u16>,
}

/// `GET /plugin-projects/{name}/terminal` — upgrade to a dev shell.
///
/// The route lives inside the guarded `/api/v1` nest; browsers send the
/// session cookie on the same-origin handshake, so no credential ever
/// reaches the page. The container is started on demand (devcontainer
/// attach semantics) when it is not already running.
pub(crate) async fn terminal(
    State(state): State<GatewayState>,
    Path(name): Path<String>,
    Query(query): Query<TerminalQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, super::error::GatewayError> {
    // Refuse unknown projects before the upgrade: a 404 response beats
    // an upgrade followed by an immediate close.
    plugins::project_workspace(&state, &name)?;
    let manager = state
        .infra
        .rsi
        .dev_sessions()
        .map_err(|error| super::error::GatewayError::Status(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            error.to_string(),
        ))?;
    let cols = query.cols.unwrap_or(80).clamp(10, 500);
    let rows = query.rows.unwrap_or(24).clamp(4, 200);
    let stream = manager
        .exec(&name, true, cols, rows)
        .await
        .map_err(|error| super::error::GatewayError::Status(
            axum::http::StatusCode::BAD_GATEWAY,
            error.to_string(),
        ))?;
    Ok(ws.on_upgrade(move |socket| bridge(socket, stream)))
}

async fn bridge(socket: WebSocket, stream: container_runtime::dev_container::DevExecStream) {
    let (socket_sender, mut socket_receiver) = socket.split();
    let container_runtime::dev_container::DevExecStream {
        stdin,
        stdout,
        stderr: _, // with --tty podman merges stderr into the pty
        exit,
    } = stream;

    // ws → stdin through a bounded channel (backpressure).
    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let stdin_task = tokio::spawn(async move {
        let mut stdin = stdin;
        while let Some(bytes) = stdin_rx.recv().await {
            if stdin.write_all(&bytes).await.is_err() {
                break;
            }
        }
        // Channel closed (socket gone): EOF so the shell exits.
        let _ = stdin.shutdown().await;
    });

    // pty → ws. When the pump ends (pty EOF — the user typed `exit`),
    // the sender task reports the exec's exit code and stops; the pane
    // shows a reconnect button instead of auto-respawning.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Message>(64);
    let pump_task = tokio::spawn(pump_read_to_channel(stdout, out_tx.clone()));
    let sender_task = tokio::spawn(async move {
        let mut sender = socket_sender;
        while let Some(message) = out_rx.recv().await {
            if sender.send(message).await.is_err() {
                return;
            }
        }
        let code = match tokio::time::timeout(std::time::Duration::from_secs(3), exit).await {
            Ok(joined) => joined.ok().and_then(|result| result.ok()),
            Err(_) => None,
        };
        let frame = serde_json::json!({ "type": "exit", "code": code });
        let _ = sender.send(Message::Text(frame.to_string().into())).await;
        let _ = sender.close().await;
    });

    // Reader loop: stdin bytes and control frames.
    while let Some(Ok(message)) = socket_receiver.next().await {
        match message {
            Message::Binary(bytes) => {
                if stdin_tx.send(bytes.to_vec()).await.is_err() {
                    break;
                }
            }
            Message::Text(text) => {
                let text = text.as_str();
                if let Ok(control) = serde_json::from_str::<serde_json::Value>(text) {
                    if control["type"] == "resize" {
                        let _ = out_tx
                            .send(Message::Text(
                                serde_json::json!({
                                    "type": "notice",
                                    "message": "the podman CLI cannot resize a live exec; open a new shell to apply the size",
                                })
                                .to_string().into(),
                            ))
                            .await;
                    }
                }
            }
            Message::Close(_) | Message::Ping(_) | Message::Pong(_) => {}
        }
    }

    // Socket closed: tear everything down. Dropping stdin_tx ends the
    // writer task with EOF (the shell exits); aborting the tasks
    // force-kills the exec child if it lingers (kill-on-drop).
    drop(stdin_tx);
    drop(out_tx);
    pump_task.abort();
    sender_task.abort();
    stdin_task.abort();
}

/// Pump an async reader into a bounded channel in 4 KiB chunks. Split
/// out so it is unit-testable with `tokio::io::duplex` (no podman).
pub(crate) async fn pump_read_to_channel<R: AsyncRead + Unpin>(
    mut reader: R,
    tx: tokio::sync::mpsc::Sender<Message>,
) -> std::io::Result<()> {
    let mut buffer = vec![0u8; 4096];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        if tx
            .send(Message::Binary(axum::body::Bytes::copy_from_slice(&buffer[..read])))
            .await
            .is_err()
        {
            // Receiver gone: stop reading.
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pump_forwards_chunks_until_eof_and_stops_on_closed_receiver() {
        use tokio::io::AsyncWriteExt;
        let (mut client, server) = tokio::io::duplex(64);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(4);
        let task = tokio::spawn(pump_read_to_channel(server, tx));

        client.write_all(b"hello ").await.unwrap();
        client.write_all(b"terminal").await.unwrap();
        // Chunk boundaries are not guaranteed (the duplex may coalesce
        // the writes); collect until the full payload is seen.
        let mut collected: Vec<u8> = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !collected.ends_with(b"terminal") {
                match rx.recv().await.unwrap() {
                    Message::Binary(bytes) => collected.extend_from_slice(&bytes[..]),
                    other => panic!("expected binary, got {other:?}"),
                }
            }
        })
        .await
        .expect("both chunks within 2s");
        assert_eq!(collected, b"hello terminal");

        // EOF on the reader ends the pump cleanly.
        drop(client);
        task.await.unwrap().unwrap();

        // A closed receiver ends the pump instead of blocking forever.
        let (mut client, server) = tokio::io::duplex(64);
        let (tx, rx) = tokio::sync::mpsc::channel::<Message>(4);
        let task = tokio::spawn(pump_read_to_channel(server, tx));
        drop(rx);
        client.write_all(b"nobody-listens").await.unwrap();
        assert!(task.await.unwrap().is_ok());
    }
}
