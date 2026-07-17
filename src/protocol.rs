#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// Commands sent from the CLI to the daemon over the IPC channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Store a plaintext secret. The daemon encrypts it internally.
    Set { secret: String },
    /// Retrieve the stored secret. Returns `NoSecret` if nothing is stored.
    Get,
    /// Query daemon status (whether a secret is held, timeout info).
    Status,
    /// Immediately clear the stored secret.
    Purge,
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
    pub has_secret: bool,
    pub last_accessed_secs_ago: u64,
    pub timeout_secs: u64,
}

impl DaemonStatus {
    pub fn no_secret(timeout_secs: u64) -> Self {
        Self {
            has_secret: false,
            last_accessed_secs_ago: 0,
            timeout_secs,
        }
    }
}
