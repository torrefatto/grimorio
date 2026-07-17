#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// Commands sent from the CLI to the daemon over the IPC channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Store a plaintext secret under `key`. The daemon encrypts it internally.
    Set { key: String, secret: String },
    /// Retrieve the secret stored under `key`. Returns `NoSecret` if none is stored.
    Get { key: String },
    /// Query daemon status (which secrets are held, timeout info).
    Status,
    /// Clear the secret under `key`, or every secret when `key` is `None`.
    Purge { key: Option<String> },
}

/// Responses sent from the daemon back to the CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Response {
    Ok,
    /// The decrypted secret as plaintext.
    Secret(String),
    /// Current daemon status.
    Status(DaemonStatus),
    /// No secret is currently stored.
    NoSecret,
    /// An error occurred.
    Error(String),
}

/// Operational state of the daemon, reported via `Command::Status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    /// True if at least one secret is stored.
    pub has_secret: bool,
    /// Number of secrets currently held.
    pub count: usize,
    /// The keys of the stored secrets, sorted.
    pub keys: Vec<String>,
    /// Elapsed seconds since the most recently accessed secret (0 if none).
    pub last_accessed_secs_ago: u64,
    pub timeout_secs: u64,
}

impl DaemonStatus {
    pub fn no_secret(timeout_secs: u64) -> Self {
        Self {
            has_secret: false,
            count: 0,
            keys: Vec::new(),
            last_accessed_secs_ago: 0,
            timeout_secs,
        }
    }
}
