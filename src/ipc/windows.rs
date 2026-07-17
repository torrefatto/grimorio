#![allow(dead_code)]

//! Windows named pipe transport.
//!
//! A named pipe server serves a fixed set of instances. To accept clients
//! continuously we keep one idle instance waiting, and the moment a client
//! connects we create the next instance before servicing the current one, so a
//! second client is never rejected while we are busy.
//!
//! NOTE: This module is only compiled on Windows and has not been built or
//! exercised on the development host (macOS). Treat it as unverified until run
//! on a Windows target.

use super::{client_exchange, serve_connection};
use crate::protocol::{Command, Response};
use std::path::Path;
use std::time::Duration;
use tokio::net::windows::named_pipe::{ClientOptions, PipeMode, ServerOptions};
use tracing::info;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY};

/// The endpoint path is used verbatim as the pipe name, e.g.
/// `\\.\pipe\grimorio-<username>`.
fn pipe_name(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Create the listening pipe and serve connections until an error occurs.
pub async fn run_server<F, Fut>(
    socket_path: &Path,
    _timeout: Duration,
    handler: F,
) -> std::io::Result<()>
where
    F: FnMut(Command) -> Fut + Clone + Send + 'static,
    Fut: std::future::Future<Output = Response> + Send + 'static,
{
    let name = pipe_name(socket_path);

    // Create the first instance up front so the pipe exists before any client
    // tries to connect. `first_pipe_instance(true)` fails if another process
    // already owns this name, preventing two daemons from clashing.
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .pipe_mode(PipeMode::Byte)
        .create(&name)?;

    info!(pipe = %name, "listening");

    loop {
        // Wait for a client to connect to the current instance.
        server.connect().await?;

        // Pre-create the next instance so the next client is not rejected while
        // we service the current one, then hand the connected instance off.
        let next = ServerOptions::new()
            .reject_remote_clients(true)
            .pipe_mode(PipeMode::Byte)
            .create(&name)?;
        let connected = std::mem::replace(&mut server, next);

        let handler = handler.clone();
        tokio::spawn(async move {
            serve_connection(connected, handler).await;
        });
    }
}

/// Connect to the daemon pipe and perform one request/response exchange.
pub async fn send_line(socket_path: &Path, request: &str) -> std::io::Result<String> {
    let name = pipe_name(socket_path);

    // All instances may be momentarily busy between a client connecting and the
    // server creating the next instance; retry briefly on ERROR_PIPE_BUSY.
    let client = loop {
        match ClientOptions::new().open(&name) {
            Ok(client) => break client,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "cannot connect: daemon not running",
                ));
            }
            Err(e) => return Err(e),
        }
    };

    client_exchange(client, request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // Windows-only round trip over a real named pipe. Only compiled and run on
    // Windows targets.
    #[tokio::test]
    async fn named_pipe_round_trip() {
        let name = format!(r"\\.\pipe\grimorio-test-{}", std::process::id());

        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .pipe_mode(PipeMode::Byte)
            .create(&name)
            .unwrap();

        let server_task = tokio::spawn(async move {
            server.connect().await.unwrap();
            serve_connection(server, |cmd: Command| async move {
                match cmd {
                    Command::Get { key } => Response::Secret(format!("value-of-{key}")),
                    _ => Response::Error("unexpected".into()),
                }
            })
            .await;
        });

        // Give the server a moment to reach connect().
        tokio::time::sleep(Duration::from_millis(50)).await;

        let request = serde_json::to_string(&Command::Get {
            key: "db".to_string(),
        })
        .unwrap();
        let response_line = send_line(Path::new(&name), &request).await.unwrap();

        let response: Response = serde_json::from_str(response_line.trim()).unwrap();
        match response {
            Response::Secret(s) => assert_eq!(s, "value-of-db"),
            other => panic!("unexpected response: {other:?}"),
        }

        server_task.await.unwrap();
    }
}
