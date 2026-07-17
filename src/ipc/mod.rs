#![allow(dead_code)]

//! IPC transport for grimorio.
//!
//! The wire protocol is one JSON line per message in each direction. The
//! transport itself is selected at compile time: Unix domain sockets on
//! Unix-like systems (`unix` submodule) and named pipes on Windows (`windows`
//! submodule). Both transports carry the same line-oriented exchange, so the
//! per-connection logic lives here, generic over any async byte stream.

use crate::protocol::{Command, Response};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tracing::error;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// Remove any filesystem artifact backing the endpoint.
///
/// On Unix this unlinks the socket file. Windows named pipes have no
/// filesystem artifact, so this is a no-op there.
pub fn cleanup_socket(path: &Path) {
    #[cfg(unix)]
    unix::cleanup_socket(path);
    #[cfg(windows)]
    let _ = path;
}

/// Run the IPC accept loop.
///
/// Each accepted connection reads one JSON line (a [`Command`]), processes it
/// via `handler`, and writes one JSON line (a [`Response`]). The transport is
/// chosen at compile time.
pub async fn run_server<F, Fut>(
    socket_path: &Path,
    timeout: Duration,
    handler: F,
) -> std::io::Result<()>
where
    F: FnMut(Command) -> Fut + Clone + Send + 'static,
    Fut: std::future::Future<Output = Response> + Send + 'static,
{
    #[cfg(unix)]
    return unix::run_server(socket_path, timeout, handler).await;
    #[cfg(windows)]
    return windows::run_server(socket_path, timeout, handler).await;
}

/// Connect to the daemon, send one request line, and read one response line.
///
/// Returns the raw response line (any trailing newline included).
pub async fn send_line(socket_path: &Path, request: &str) -> std::io::Result<String> {
    #[cfg(unix)]
    return unix::send_line(socket_path, request).await;
    #[cfg(windows)]
    return windows::send_line(socket_path, request).await;
}

/// Serve a single already-accepted connection.
///
/// Reads command lines, dispatches each to `handler`, and writes the response
/// line. Generic over any async byte stream so the same logic drives Unix
/// sockets and Windows named pipes.
pub(crate) async fn serve_connection<S, F, Fut>(stream: S, mut handler: F)
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnMut(Command) -> Fut,
    Fut: std::future::Future<Output = Response>,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Command>(trimmed) {
            Ok(cmd) => handler(cmd).await,
            Err(e) => {
                error!(?e, "failed to parse command");
                Response::Error(format!("invalid command: {e}"))
            }
        };

        let response_line = match serde_json::to_string(&response) {
            Ok(s) => s,
            Err(e) => {
                error!(?e, "failed to serialize response");
                return;
            }
        };

        if writer.write_all(response_line.as_bytes()).await.is_err() {
            return;
        }
        if writer.write_all(b"\n").await.is_err() {
            return;
        }
        if writer.flush().await.is_err() {
            return;
        }
    }
}

/// Client-side exchange over an established stream: write one request line, read
/// one response line. Generic over the transport stream.
pub(crate) async fn client_exchange<S>(stream: S, request: &str) -> std::io::Result<String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (reader, mut writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);

    writer.write_all(request.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    let mut line = String::new();
    reader.read_line(&mut line).await?;
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Drives `serve_connection` over an in-memory duplex stream, exercising the
    // transport-agnostic request/response path without any real socket.
    #[tokio::test]
    async fn serve_connection_handles_one_command() {
        let (client, server) = tokio::io::duplex(1024);

        let server_task = tokio::spawn(async move {
            serve_connection(server, |cmd: Command| async move {
                match cmd {
                    Command::Get { key } => Response::Secret(format!("value-of-{key}")),
                    _ => Response::Error("unexpected".into()),
                }
            })
            .await;
        });

        let request = serde_json::to_string(&Command::Get {
            key: "db".to_string(),
        })
        .unwrap();
        let response_line = client_exchange(client, &request).await.unwrap();

        let response: Response = serde_json::from_str(response_line.trim()).unwrap();
        match response {
            Response::Secret(s) => assert_eq!(s, "value-of-db"),
            other => panic!("unexpected response: {other:?}"),
        }

        server_task.await.unwrap();
    }
}
