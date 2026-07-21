mod crypto;
mod ipc;
mod password;
mod protocol;

use clap::Parser;
use crypto::{decrypt, encrypt, generate_key};
use password::{PasswordReader, TerminalPasswordReader};
use protocol::{Command, DaemonStatus, Response};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use zeroize::Zeroize;

/// A daemon that keeps a secret encrypted in memory.
#[derive(Parser)]
#[command(name = "grimoriod", version, about)]
struct Cli {
    /// Path to the Unix socket. Overrides GRIMORIO_SOCKET env var.
    #[arg(long, env = "GRIMORIO_SOCKET", default_value_os = socket_socket_path().to_str().unwrap())]
    socket: PathBuf,

    /// Seconds before the daemon purges an idle secret.
    #[arg(long, default_value = "300")]
    timeout: u64,
}

/// One stored secret: its encrypted blob and the time it was last accessed.
struct SecretEntry {
    /// The encrypted secret blob.
    blob: Vec<u8>,
    /// Timestamp of the last successful decrypt (i.e. last `Get`) or store.
    last_accessed: Instant,
}

/// All mutable daemon state, protected by a mutex for concurrent access.
struct DaemonState {
    /// The AES-256 encryption key, generated at startup.
    key: [u8; 32],
    /// The stored secrets, keyed by name.
    secrets: HashMap<String, SecretEntry>,
    /// How long a secret may live without being accessed.
    timeout: Duration,
}

impl DaemonState {
    fn new(key: [u8; 32], timeout: Duration) -> Self {
        Self {
            key,
            secrets: HashMap::new(),
            timeout,
        }
    }

    fn status(&self) -> DaemonStatus {
        let mut keys: Vec<String> = self.secrets.keys().cloned().collect();
        keys.sort();

        let last_accessed_secs_ago = self
            .secrets
            .values()
            .map(|e| e.last_accessed.elapsed().as_secs())
            .min()
            .unwrap_or(0);

        DaemonStatus {
            has_secret: !self.secrets.is_empty(),
            count: self.secrets.len(),
            keys,
            last_accessed_secs_ago,
            timeout_secs: self.timeout.as_secs(),
        }
    }

    /// Remove and zeroize every secret that has outlived the idle timeout.
    /// Returns the number of secrets purged.
    fn purge_expired(&mut self) -> usize {
        let timeout = self.timeout;
        let expired: Vec<String> = self
            .secrets
            .iter()
            .filter(|(_, e)| e.last_accessed.elapsed() > timeout)
            .map(|(k, _)| k.clone())
            .collect();

        for key in &expired {
            if let Some(mut entry) = self.secrets.remove(key) {
                entry.blob.zeroize();
            }
        }
        expired.len()
    }

    /// Remove and zeroize every stored secret.
    fn purge_all(&mut self) {
        for entry in self.secrets.values_mut() {
            entry.blob.zeroize();
        }
        self.secrets.clear();
    }
}

/// Handle a single command. Pure function of current state -> (response, new_state).
fn handle_command(state: &mut DaemonState, cmd: Command, _reader: &dyn PasswordReader) -> Response {
    match cmd {
        Command::Set { key, secret } => {
            let blob = encrypt(&state.key, secret.as_bytes());
            state.secrets.insert(
                key.clone(),
                SecretEntry {
                    blob,
                    last_accessed: Instant::now(),
                },
            );
            info!(key = %key, "secret stored");
            Response::Ok
        }

        Command::Get { key } => {
            let daemon_key = state.key;
            match state.secrets.get_mut(&key) {
                Some(entry) => match decrypt(&daemon_key, &entry.blob) {
                    Ok(plaintext) => {
                        entry.last_accessed = Instant::now();
                        // SAFETY: plaintext was originally a String, so it's valid UTF-8.
                        let s = String::from_utf8(plaintext).unwrap_or_else(|e| {
                            warn!("decrypted bytes are not valid UTF-8: {}", e);
                            String::from_utf8_lossy(e.as_bytes()).into_owned()
                        });
                        info!(key = %key, "secret retrieved");
                        Response::Secret(s)
                    }
                    Err(e) => {
                        error!(?e, "decryption failed");
                        Response::Error(format!("decryption failed: {e}"))
                    }
                },
                // The daemon is detached and has no terminal, so it never prompts.
                // It reports the absence of a secret; the CLI owns any interaction.
                None => Response::NoSecret,
            }
        }

        Command::Status => Response::Status(state.status()),

        Command::Purge { key } => match key {
            Some(key) => match state.secrets.remove(&key) {
                Some(mut entry) => {
                    entry.blob.zeroize();
                    info!(key = %key, "secret purged");
                    Response::Ok
                }
                None => Response::NoSecret,
            },
            None => {
                state.purge_all();
                info!("all secrets purged");
                Response::Ok
            }
        },
    }
}

/// Background task: periodically check if the secret has expired and purge it.
async fn timeout_monitor(state: Arc<Mutex<DaemonState>>, check_interval: Duration) {
    loop {
        tokio::time::sleep(check_interval).await;

        let mut s = state.lock().await;
        let purged = s.purge_expired();
        if purged > 0 {
            info!(count = purged, "expired secrets purged");
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("grimorio=info".parse()?),
        )
        .init();

    let cli = Cli::parse();
    let socket_path = cli.socket;
    let timeout = Duration::from_secs(cli.timeout);

    info!("starting grimoriod");
    info!(socket = %socket_path.display(), timeout_secs = cli.timeout, "configuration");

    let key = generate_key();
    let state = Arc::new(Mutex::new(DaemonState::new(key, timeout)));

    // Spawn the timeout monitor.
    let monitor_state = Arc::clone(&state);
    tokio::spawn(timeout_monitor(monitor_state, Duration::from_secs(1)));

    // Spawn the IPC server.
    let server_state = Arc::clone(&state);
    let server_socket = socket_path.clone();
    tokio::spawn(async move {
        let handler = move |cmd: Command| {
            let state = Arc::clone(&server_state);
            let reader: Box<dyn PasswordReader> = Box::new(TerminalPasswordReader);
            async move {
                let mut s = state.lock().await;
                handle_command(&mut s, cmd, reader.as_ref())
            }
        };

        if let Err(e) = ipc::run_server(&server_socket, timeout, handler).await {
            error!(?e, "IPC server error");
        }
    });

    // Wait for SIGTERM / SIGINT and shut down.
    wait_for_shutdown_signal().await;
    info!("shutting down");

    // Zeroize every stored secret before exit.
    {
        let mut s = state.lock().await;
        s.purge_all();
    }

    ipc::cleanup_socket(&socket_path);
    Ok(())
}

/// Resolve the default IPC endpoint: GRIMORIO_SOCKET if set, otherwise a Unix
/// socket under ~/.grimorio (Unix) or a per-user named pipe (Windows).
fn socket_socket_path() -> &'static std::ffi::OsStr {
    static PATH: std::sync::OnceLock<std::ffi::OsString> = std::sync::OnceLock::new();
    let path = PATH.get_or_init(|| {
        if let Ok(val) = std::env::var("GRIMORIO_SOCKET") {
            return PathBuf::from(val).into_os_string();
        }
        #[cfg(unix)]
        let p = {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".grimorio").join("grimorio.sock")
        };
        #[cfg(windows)]
        let p = {
            let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".to_string());
            PathBuf::from(format!(r"\\.\pipe\grimorio-{user}"))
        };
        p.into_os_string()
    });
    path.as_os_str()
}

/// Wait for SIGTERM (Unix) or Ctrl+C (all platforms).
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm =
            signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => { info!("received SIGINT"); }
            _ = sigterm.recv() => { info!("received SIGTERM"); }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to listen for Ctrl+C");
        info!("received Ctrl+C");
    }
}
