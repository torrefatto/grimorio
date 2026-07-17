mod crypto;
mod ipc;
mod password;
mod protocol;

use clap::Parser;
use crypto::{decrypt, encrypt, generate_key};
use password::{PasswordReader, TerminalPasswordReader};
use protocol::{Command, DaemonStatus, Response};
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

/// All mutable daemon state, protected by a mutex for concurrent access.
struct DaemonState {
    /// The AES-256 encryption key, generated at startup.
    key: [u8; 32],
    /// The encrypted secret blob, if one is stored.
    secret: Option<Vec<u8>>,
    /// Timestamp of the last successful decrypt (i.e. last `Get`).
    last_accessed: Option<Instant>,
    /// How long a secret may live without being accessed.
    timeout: Duration,
}

impl DaemonState {
    fn new(key: [u8; 32], timeout: Duration) -> Self {
        Self {
            key,
            secret: None,
            last_accessed: None,
            timeout,
        }
    }

    fn status(&self) -> DaemonStatus {
        let (has_secret, last_accessed_secs_ago) = match (&self.secret, self.last_accessed) {
            (Some(_), Some(instant)) => (true, instant.elapsed().as_secs()),
            _ => (false, 0),
        };

        DaemonStatus {
            has_secret,
            last_accessed_secs_ago,
            timeout_secs: self.timeout.as_secs(),
        }
    }

    fn is_expired(&self) -> bool {
        match self.last_accessed {
            Some(instant) => instant.elapsed() > self.timeout,
            None => false,
        }
    }
}

/// Handle a single command. Pure function of current state -> (response, new_state).
fn handle_command(
    state: &mut DaemonState,
    cmd: Command,
    _reader: &dyn PasswordReader,
) -> Response {
    match cmd {
        Command::Set { secret } => {
            let blob = encrypt(&state.key, secret.as_bytes());
            state.secret = Some(blob);
            state.last_accessed = Some(Instant::now());
            info!("secret stored");
            Response::Ok
        }

        Command::Get => match &state.secret {
            Some(blob) => match decrypt(&state.key, blob) {
                Ok(plaintext) => {
                    state.last_accessed = Some(Instant::now());
                    // SAFETY: plaintext was originally a String, so it's valid UTF-8.
                    let s = String::from_utf8(plaintext).unwrap_or_else(|e| {
                        warn!("decrypted bytes are not valid UTF-8: {}", e);
                        String::from_utf8_lossy(e.as_bytes()).into_owned()
                    });
                    info!("secret retrieved");
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
        },

        Command::Status => Response::Status(state.status()),

        Command::Purge => {
            if let Some(ref mut blob) = state.secret {
                blob.zeroize();
            }
            state.secret = None;
            state.last_accessed = None;
            info!("secret purged");
            Response::Ok
        }
    }
}

/// Background task: periodically check if the secret has expired and purge it.
async fn timeout_monitor(state: Arc<Mutex<DaemonState>>, check_interval: Duration) {
    loop {
        tokio::time::sleep(check_interval).await;

        let mut s = state.lock().await;
        if s.is_expired() {
            if let Some(ref mut blob) = s.secret {
                blob.zeroize();
            }
            s.secret = None;
            s.last_accessed = None;
            info!("secret expired and was purged");
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

    // Zeroize the key before exit.
    {
        let mut s = state.lock().await;
        if let Some(ref mut blob) = s.secret {
            blob.zeroize();
        }
        s.secret = None;
        s.last_accessed = None;
    }

    ipc::cleanup_socket(&socket_path);
    Ok(())
}

/// Resolve the socket path: GRIMORIO_SOCKET env var, then ~/.grimorio/grimorio.sock.
fn socket_socket_path() -> &'static std::ffi::OsStr {
    static PATH: std::sync::OnceLock<std::ffi::OsString> = std::sync::OnceLock::new();
    let path = PATH.get_or_init(|| {
        let p = if let Ok(val) = std::env::var("GRIMORIO_SOCKET") {
            PathBuf::from(val)
        } else {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".grimorio").join("grimorio.sock")
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
