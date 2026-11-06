//! HTTP and upgrade transport for a VS Code server over a private socket.
//!
//! The browser listener uses a separate loopback host and a project capability.

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use hyper_util::rt::TokioIo;
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::net::UnixStream;

use super::error::{GatewayError, GatewayResult};

/// Forward a request to an already-connected project socket.
///
/// Authentication belongs to the eventual isolated-origin listener. Never
/// pass its credentials to a server controlled by the project container.
pub(crate) async fn forward(stream: UnixStream, request: Request) -> GatewayResult<Response> {
    forward_with_prefix(stream, request, None).await
}

async fn forward_with_prefix(
    stream: UnixStream,
    mut request: Request,
    prefix: Option<&str>,
) -> GatewayResult<Response> {
    for name in [
        header::AUTHORIZATION,
        header::PROXY_AUTHORIZATION,
        header::COOKIE,
        header::FORWARDED,
    ] {
        request.headers_mut().remove(name);
    }
    request.headers_mut().remove("x-original-host");
    for name in [
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-port",
        "x-forwarded-proto",
        "x-forwarded-prefix",
    ] {
        request.headers_mut().remove(name);
    }
    if let Some(prefix) = prefix {
        request.headers_mut().insert(
            "x-forwarded-prefix",
            prefix.parse().map_err(|_| bad_gateway())?,
        );
    }

    let requested_upgrade = request.headers().get(header::UPGRADE).is_some();
    let client_upgrade = requested_upgrade.then(|| hyper::upgrade::on(&mut request));
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|_| bad_gateway())?;
    tokio::spawn(async move {
        if let Err(error) = connection.with_upgrades().await {
            tracing::debug!(%error, "VS Code socket connection ended");
        }
    });
    let mut response = sender
        .send_request(request)
        .await
        .map_err(|_| bad_gateway())?;
    // A container must not set cookies for the gateway's browser origin.
    response.headers_mut().remove(header::SET_COOKIE);
    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        let Some(client_upgrade) = client_upgrade else {
            return Err(bad_gateway());
        };
        let server_upgrade = hyper::upgrade::on(&mut response);
        tokio::spawn(async move {
            if let (Ok(client), Ok(server)) = (client_upgrade.await, server_upgrade.await) {
                let (mut client, mut server) = (TokioIo::new(client), TokioIo::new(server));
                let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
            }
        });
    }
    Ok(response.map(Body::new))
}

#[derive(Clone)]
struct ListenerState {
    socket: PathBuf,
    capability: String,
    base: String,
    host: String,
}

pub(crate) struct Listener {
    pub(crate) url: String,
    pub(crate) task: tokio::task::JoinHandle<()>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Bind only a loopback address distinct from the default gateway hostname.
pub(crate) async fn listen(
    socket: PathBuf,
    base: String,
    port_file: &Path,
) -> GatewayResult<Listener> {
    let listener = match read_port(port_file)? {
        Some(port) => bind_port(port).await?,
        None => {
            let candidate = bind_port(0).await?;
            let port = candidate.local_addr().map_err(|_| bad_gateway())?.port();
            let parent = port_file.parent().ok_or_else(bad_gateway)?;
            let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| bad_gateway())?;
            file.write_all(port.to_string().as_bytes())
                .map_err(|_| bad_gateway())?;
            file.as_file().sync_all().map_err(|_| bad_gateway())?;
            match file.persist_noclobber(port_file) {
                Ok(_) => candidate,
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    drop(candidate);
                    bind_port(read_port(port_file)?.ok_or_else(bad_gateway)?).await?
                }
                Err(_) => return Err(bad_gateway()),
            }
        }
    };
    let port = listener.local_addr().map_err(|_| bad_gateway())?.port();
    let capability = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let host = format!("127.0.0.2:{port}");
    let url = format!("http://{host}/{capability}{base}/?folder=%2Fwork");
    let state = Arc::new(ListenerState {
        socket,
        capability,
        base,
        host,
    });
    let app = Router::new().fallback(serve).with_state(state);
    let task = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            tracing::warn!(%error, "VS Code local listener stopped");
        }
    });
    Ok(Listener { url, task })
}

fn read_port(path: &Path) -> GatewayResult<Option<u16>> {
    use std::os::unix::fs::MetadataExt;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(bad_gateway()),
    };
    // SAFETY: geteuid has no preconditions and only reads process identity.
    let uid = unsafe { libc::geteuid() };
    if !metadata.file_type().is_file()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
        || metadata.len() > 5
    {
        return Err(GatewayError::Status(
            StatusCode::SERVICE_UNAVAILABLE,
            "VS Code 端口记录无效或权限不安全".into(),
        ));
    }
    let port = std::fs::read_to_string(path)
        .map_err(|_| bad_gateway())?
        .parse::<u16>()
        .ok()
        .filter(|port| *port >= 1024)
        .ok_or_else(|| {
            GatewayError::Status(
                StatusCode::SERVICE_UNAVAILABLE,
                "VS Code 端口记录无效".into(),
            )
        })?;
    Ok(Some(port))
}

async fn bind_port(port: u16) -> GatewayResult<tokio::net::TcpListener> {
    for attempt in 0..20 {
        match tokio::net::TcpListener::bind((std::net::Ipv4Addr::new(127, 0, 0, 2), port)).await {
            Ok(listener) => return Ok(listener),
            Err(error)
                if port != 0 && error.kind() == std::io::ErrorKind::AddrInUse && attempt < 19 =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => {
                return Err(GatewayError::Status(
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!("无法绑定 VS Code 本地端口 {port}：{error}"),
                ));
            }
        }
    }
    Err(bad_gateway())
}

async fn serve(State(state): State<Arc<ListenerState>>, mut request: Request) -> Response {
    let host_ok = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        == Some(&state.host);
    let origin_ok = if request.headers().contains_key(header::UPGRADE) {
        request
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            == Some(format!("http://{}", state.host).as_str())
    } else {
        true
    };
    let secret_prefix = format!("/{}", state.capability);
    let expected = format!("{secret_prefix}{}", state.base);
    let path = request.uri().path();
    if !host_ok
        || !origin_ok
        || !(path == expected
            || path
                .strip_prefix(&expected)
                .is_some_and(|rest| rest.starts_with('/')))
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let original = request
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let Some(relative) = original.strip_prefix(&secret_prefix) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(uri) = relative.parse() else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    *request.uri_mut() = uri;
    let stream = match UnixStream::connect(&state.socket).await {
        Ok(stream) => stream,
        Err(_) => return bad_gateway().into_response(),
    };
    match forward_with_prefix(stream, request, Some(&expected)).await {
        Ok(mut response) => {
            response.headers_mut().insert(
                header::REFERRER_POLICY,
                "no-referrer".parse().expect("static header"),
            );
            response
        }
        Err(error) => error.into_response(),
    }
}

fn bad_gateway() -> GatewayError {
    GatewayError::Status(StatusCode::BAD_GATEWAY, "VS Code 容器服务不可用".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn preserves_project_origin_and_rejects_occupied_port() {
        let dir = tempfile::tempdir().unwrap();
        let port_file = dir.path().join("port");
        let socket = dir.path().join("socket");
        let first = listen(socket.clone(), "/base".into(), &port_file)
            .await
            .unwrap_or_else(|_| panic!("first listener"));
        let origin = first.url.split('/').take(3).collect::<Vec<_>>().join("/");
        assert!(
            listen(socket.clone(), "/base".into(), &port_file)
                .await
                .is_err()
        );
        let old_url = first.url.clone();
        drop(first);
        let second = listen(socket, "/base".into(), &port_file)
            .await
            .unwrap_or_else(|_| panic!("reopened listener"));
        assert_eq!(
            second.url.split('/').take(3).collect::<Vec<_>>().join("/"),
            origin
        );
        assert_ne!(old_url, second.url);
        use std::os::unix::fs::symlink;
        let link = dir.path().join("link");
        symlink(&port_file, &link).unwrap();
        assert!(read_port(&link).is_err());
    }

    #[tokio::test]
    async fn removes_credentials_and_response_cookies() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("server.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut input = vec![0; 8192];
            let size = stream.read(&mut input).await.unwrap();
            let input = String::from_utf8_lossy(&input[..size]).to_ascii_lowercase();
            assert!(input.starts_with("get /base/path?q=1 http/1.1\r\n"));
            for forbidden in [
                "authorization:",
                "cookie:",
                "forwarded:",
                "x-forwarded-host:",
            ] {
                assert!(!input.contains(forbidden), "{input}");
            }
            assert!(input.contains("x-test: kept"));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nSet-Cookie: malicious=1\r\n\r\nok",
                )
                .await
                .unwrap();
        });
        let request = Request::builder()
            .uri("/base/path?q=1")
            .header("host", "localhost")
            .header("authorization", "Bearer secret")
            .header("proxy-authorization", "secret")
            .header("cookie", "autonomics_session=secret")
            .header("forwarded", "host=untrusted")
            .header("x-forwarded-host", "untrusted")
            .header("x-test", "kept")
            .body(Body::empty())
            .unwrap();
        let response = forward(UnixStream::connect(&socket).await.unwrap(), request)
            .await
            .unwrap_or_else(|_| panic!("forward request"));
        assert_eq!(response.status(), StatusCode::OK);
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        let bytes = axum::body::to_bytes(response.into_body(), 16)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"ok");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn replaces_untrusted_forwarded_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("server.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut input = [0; 8192];
            let size = stream.read(&mut input).await.unwrap();
            let input = String::from_utf8_lossy(&input[..size]).to_ascii_lowercase();
            assert!(input.contains("x-forwarded-prefix: /trusted/base\r\n"));
            assert!(!input.contains("/untrusted"));
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
                .await
                .unwrap();
        });
        let request = Request::builder()
            .uri("/")
            .header("host", "localhost")
            .header("x-forwarded-prefix", "/untrusted")
            .body(Body::empty())
            .unwrap();
        let response = forward_with_prefix(
            UnixStream::connect(&socket).await.unwrap(),
            request,
            Some("/trusted/base"),
        )
        .await
        .unwrap_or_else(|_| panic!("forward request"));
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn relays_upgraded_bytes_in_both_directions() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("server.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.unwrap());
            }
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n").await.unwrap();
            let mut bytes = [0; 4];
            stream.read_exact(&mut bytes).await.unwrap();
            assert_eq!(&bytes, b"ping");
            stream.write_all(b"pong").await.unwrap();
        });
        let app = axum::Router::new().route(
            "/",
            axum::routing::get(move |request: Request| {
                let socket = socket.clone();
                async move {
                    let stream = UnixStream::connect(socket).await.unwrap();
                    forward(stream, request)
                        .await
                        .unwrap_or_else(|_| panic!("forward upgrade"))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let http_server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        client.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n").await.unwrap();
        let mut headers = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !headers.ends_with(b"\r\n\r\n") {
                headers.push(client.read_u8().await.unwrap());
            }
        })
        .await
        .unwrap();
        assert!(headers.starts_with(b"HTTP/1.1 101"));
        client.write_all(b"ping").await.unwrap();
        let mut reply = [0; 4];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut reply),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(&reply, b"pong");
        server.await.unwrap();
        http_server.abort();
    }
}
