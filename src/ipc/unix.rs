#![allow(dead_code)]

//! Unix domain socket transport.

use super::{client_exchange, serve_connection};
use crate::protocol::{Command, Response};
use std::path::Path;
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};
use tracing::info;

/// Remove a stale socket file if it exists.
pub fn cleanup_socket(path: &Path) {
    if path.exists() {
        if let Err(e) = std::fs::remove_file(path) {
            // The socket may be a dangling symlink or permission-protected;
            // log and continue.
            tracing::error!(?e, path = %path.display(), "failed to remove stale socket");
        }
    }
}

/// Bind the Unix socket and serve connections until an error occurs.
pub async fn run_server<F, Fut>(
    socket_path: &Path,
    _timeout: Duration,
    handler: F,
) -> std::io::Result<()>
where
    F: FnMut(Command) -> Fut + Clone + Send + 'static,
    Fut: std::future::Future<Output = Response> + Send + 'static,
{
    cleanup_socket(socket_path);

    let listener = UnixListener::bind(socket_path)?;

    // Restrict socket permissions to owner-only (0600).
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    }

    info!(path = %socket_path.display(), "listening");

    loop {
        let (stream, _) = listener.accept().await?;
        let handler = handler.clone();
        tokio::spawn(async move {
            serve_connection(stream, handler).await;
        });
    }
}

/// Connect to the daemon socket and perform one request/response exchange.
pub async fn send_line(socket_path: &Path, request: &str) -> std::io::Result<String> {
    let stream = UnixStream::connect(socket_path).await?;
    client_exchange(stream, request).await
}
