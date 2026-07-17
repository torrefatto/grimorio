mod crypto;
mod ipc;
mod password;
mod protocol;

use clap::{Parser, Subcommand};
use ipc::cleanup_socket;
use password::{PasswordReader, TerminalPasswordReader};
use protocol::{Command, Response};
use std::io::Read;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tracing::error;

/// A daemon that keeps a secret encrypted in memory.
#[derive(Parser)]
#[command(name = "grimorio", version, about)]
struct Cli {
    /// Path to the daemon's Unix socket. Overrides GRIMORIO_SOCKET env var.
    #[arg(long, env = "GRIMORIO_SOCKET", default_value_os = default_socket_path())]
    socket: PathBuf,

    /// Seconds before the daemon purges an idle secret.
    #[arg(long, default_value = "300")]
    timeout: u64,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Store a secret under KEY (reads the secret from stdin).
    Set {
        /// The key to store the secret under.
        key: String,
    },
    /// Retrieve the secret stored under KEY (prints to stdout).
    Get {
        /// The key of the secret to retrieve.
        key: String,
    },
    /// Show daemon status.
    Status,
    /// Clear the secret under KEY, or every secret when KEY is omitted.
    Purge {
        /// The key to clear. If omitted, clears all stored secrets.
        key: Option<String>,
    },
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("grimorio=info".parse().unwrap()),
        )
        // Logs and errors go to stderr; stdout is reserved for command output
        // (e.g. the retrieved secret).
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    if let Err(e) = dispatch(&cli) {
        error!("{e}");
        std::process::exit(1);
    }
}

fn dispatch(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    match &cli.command {
        Commands::Set { key } => cmd_set(&cli.socket, key),
        Commands::Get { key } => cmd_get(&cli.socket, key),
        Commands::Status => cmd_status(&cli.socket),
        Commands::Purge { key } => cmd_purge(&cli.socket, key.clone()),
    }
}

// ---------------------------------------------------------------------------
// IPC helpers
// ---------------------------------------------------------------------------

fn send_command(socket: &PathBuf, cmd: &Command) -> Result<Response, String> {
    let rt = tokio::runtime::Runtime::new().map_err(|e| format!("tokio: {e}"))?;

    rt.block_on(async {
        let stream =
            tokio::net::UnixStream::connect(socket)
                .await
                .map_err(|e| {
                    cleanup_socket(socket);
                    format!("cannot connect to daemon at {}: {e}", socket.display())
                })?;

        let (reader, mut writer) = stream.into_split();
        let mut reader = tokio::io::BufReader::new(reader);

        // Send command.
        let line = serde_json::to_string(cmd).unwrap();
        writer
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("write error: {e}"))?;
        writer
            .write_all(b"\n")
            .await
            .map_err(|e| format!("write error: {e}"))?;
        writer
            .flush()
            .await
            .map_err(|e| format!("flush error: {e}"))?;

        // Read response.
        use tokio::io::AsyncBufReadExt;
        let mut response_line = String::new();
        reader
            .read_line(&mut response_line)
            .await
            .map_err(|e| format!("read error: {e}"))?;

        serde_json::from_str(&response_line).map_err(|e| format!("invalid response: {e}"))
    })
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_set(socket: &PathBuf, key: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut secret = String::new();
    std::io::stdin()
        .read_to_string(&mut secret)
        .map_err(|e| format!("failed to read stdin: {e}"))?;

    // Trim trailing newline(s) that may have been added by pipe.
    let secret = secret.trim_end_matches('\n').trim_end_matches('\r').to_string();

    let response = send_command(
        socket,
        &Command::Set {
            key: key.to_string(),
            secret,
        },
    )?;
    match response {
        Response::Ok => {
            println!("OK");
            Ok(())
        }
        Response::Error(msg) => Err(msg.into()),
        other => Err(format!("unexpected response: {other:?}").into()),
    }
}

fn cmd_get(socket: &PathBuf, key: &str) -> Result<(), Box<dyn std::error::Error>> {
    let response = send_command(
        socket,
        &Command::Get {
            key: key.to_string(),
        },
    )?;
    match response {
        Response::Secret(s) => {
            println!("{s}");
            Ok(())
        }
        Response::NoSecret => {
            // Nothing stored under this key yet. Prompt here -- the CLI is the
            // process attached to the terminal -- cache it in the daemon under
            // the same key for future gets, then print the secret we just read.
            let reader = TerminalPasswordReader;
            let secret = reader
                .read_password(&format!("No secret stored for '{key}'. Enter secret: "))
                .map_err(|e| format!("failed to read secret: {e}"))?;

            match send_command(
                socket,
                &Command::Set {
                    key: key.to_string(),
                    secret: secret.clone(),
                },
            )? {
                Response::Ok => {
                    println!("{secret}");
                    Ok(())
                }
                Response::Error(msg) => Err(msg.into()),
                other => Err(format!("unexpected response: {other:?}").into()),
            }
        }
        Response::Error(msg) => Err(msg.into()),
        other => Err(format!("unexpected response: {other:?}").into()),
    }
}

fn cmd_status(socket: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let response = send_command(socket, &Command::Status)?;
    match response {
        Response::Status(status) => {
            println!("has_secret: {}", status.has_secret);
            println!("count: {}", status.count);
            println!("keys: {}", status.keys.join(", "));
            println!("last_accessed_secs_ago: {}", status.last_accessed_secs_ago);
            println!("timeout_secs: {}", status.timeout_secs);
            Ok(())
        }
        Response::Error(msg) => Err(msg.into()),
        other => Err(format!("unexpected response: {other:?}").into()),
    }
}

fn cmd_purge(socket: &PathBuf, key: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let missing = key.clone();
    let response = send_command(socket, &Command::Purge { key })?;
    match response {
        Response::Ok => {
            println!("OK");
            Ok(())
        }
        Response::NoSecret => match missing {
            Some(k) => Err(format!("no secret stored for '{k}'").into()),
            None => Err("no secret stored".into()),
        },
        Response::Error(msg) => Err(msg.into()),
        other => Err(format!("unexpected response: {other:?}").into()),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_socket_path() -> &'static std::ffi::OsStr {
    static PATH: std::sync::OnceLock<std::ffi::OsString> = std::sync::OnceLock::new();
    let path = PATH.get_or_init(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".grimorio")
            .join("grimorio.sock")
            .into_os_string()
    });
    path.as_os_str()
}
