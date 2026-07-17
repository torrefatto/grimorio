#![allow(dead_code)]

use crate::protocol::{Command, Response};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tracing::{error, info};

/// Remove a stale socket file if it exists.
pub fn cleanup_socket(path: &Path) {
    if path.exists() {
        if let Err(e) = std::fs::remove_file(path) {
            // On some platforms the socket may be a dangling symlink or
            // permission-protected; log and continue.
            error!(?e, path = %path.display(), "failed to remove stale socket");
        }
    }
}

/// Run the IPC accept loop on a Unix domain socket.
///
/// Each accepted connection reads one JSON line (a `Command`), processes it,
/// and writes one JSON line (a `Response`).
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    }

    info!(path = %socket_path.display(), "listening");

    loop {
        let (stream, _) = listener.accept().await?;

        let handler = handler.clone();
        tokio::spawn(async move {
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let mut handler = handler;

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
        });
    }
}
