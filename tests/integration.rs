// These tests drive the real daemon over a Unix domain socket and use
// Unix-only APIs (socket file permissions). The Windows named-pipe transport is
// covered by unit tests in `src/ipc/windows.rs`.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct DaemonFixture {
    child: Child,
    sock: PathBuf,
}

impl DaemonFixture {
    fn start() -> Self {
        let sock = temp_socket_path();
        cleanup_socket(&sock);

        let child = Command::new(daemon_bin())
            .arg("--socket")
            .arg(&sock)
            .arg("--timeout")
            .arg("300")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to start daemon");

        let start = std::time::Instant::now();
        loop {
            if sock.exists() {
                break;
            }
            if start.elapsed() > Duration::from_secs(5) {
                panic!("daemon socket never appeared at {}", sock.display());
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600)).ok();
        Self { child, sock }
    }

    fn socket(&self) -> &PathBuf {
        &self.sock
    }

    fn run(&self, args: &[&str]) -> CommandResult {
        let output = Command::new(cli_bin())
            .arg("--socket")
            .arg(&self.sock)
            .args(args)
            .output()
            .expect("failed to run CLI");

        CommandResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            status: output.status.code(),
        }
    }

    /// Store `secret` under `key` by piping it to `grimorio set KEY`.
    fn set(&self, key: &str, secret: &str) {
        let output = Command::new("sh")
            .arg("-c")
            .arg(format!(
                "printf '%s' '{}' | {} --socket {} set {}",
                secret,
                cli_bin().display(),
                self.sock.display(),
                key,
            ))
            .output()
            .expect("failed to run set via shell");

        assert!(
            output.status.success(),
            "set: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

impl Drop for DaemonFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        std::thread::sleep(Duration::from_millis(100));
        cleanup_socket(&self.sock);
    }
}

fn cleanup_socket(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
}

struct CommandResult {
    stdout: String,
    stderr: String,
    status: Option<i32>,
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn daemon_bin() -> PathBuf {
    workspace_root().join("target/debug/grimoriod")
}

fn cli_bin() -> PathBuf {
    workspace_root().join("target/debug/grimorio")
}

fn temp_socket_path() -> PathBuf {
    // Cargo runs tests as threads in a single process, so keying only on the
    // PID would make every test share one socket. Add a per-call counter so
    // each daemon gets an isolated socket.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut p = std::env::temp_dir();
    let id = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    p.push(format!("grimorio-test-{id}-{n}.sock"));
    p
}

#[test]
fn status_empty_daemon() {
    let daemon = DaemonFixture::start();
    let result = daemon.run(&["status"]);
    assert!(result.status.is_some() && result.status.unwrap() == 0, "status: {}", result.stderr);
    assert!(result.stdout.contains("has_secret: false"));
}

#[test]
fn set_and_get_secret() {
    let daemon = DaemonFixture::start();

    let secret = "my-super-secret";
    daemon.set("db", secret);

    let result = daemon.run(&["get", "db"]);
    assert!(result.status.is_some() && result.status.unwrap() == 0, "get: {}", result.stderr);
    assert_eq!(result.stdout.trim(), secret);
}

#[test]
fn get_when_no_secret_prompts_and_returns() {
    let daemon = DaemonFixture::start();

    let secret = "prompted-secret";
    let result = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "printf '%s' '{}' | {} --socket {} get api",
            secret,
            cli_bin().display(),
            daemon.socket().display()
        ))
        .output()
        .expect("failed to run get via shell");

    assert!(result.status.success(), "get: {}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), secret);

    // The prompted secret should now be cached under that key.
    let cached = daemon.run(&["get", "api"]);
    assert_eq!(cached.stdout.trim(), secret);
}

#[test]
fn get_evaluates_source_when_no_secret() {
    let daemon = DaemonFixture::start();

    // No secret stored for 'api'; the source command is evaluated and its
    // stdout is used, cached, and printed.
    let result = daemon.run(&["get", "api", "printf sourced-secret"]);
    assert!(
        result.status.is_some() && result.status.unwrap() == 0,
        "get: {}",
        result.stderr
    );
    assert_eq!(result.stdout.trim(), "sourced-secret");

    // The evaluated secret is now cached; a plain get returns it and must not
    // re-run the source.
    let cached = daemon.run(&["get", "api", "printf different"]);
    assert_eq!(cached.stdout.trim(), "sourced-secret");
}

#[test]
fn get_source_ignored_when_secret_present() {
    let daemon = DaemonFixture::start();

    daemon.set("db", "stored-secret");

    // A source is supplied but a secret already exists, so the source is not run.
    let result = daemon.run(&["get", "db", "printf should-not-run"]);
    assert_eq!(result.stdout.trim(), "stored-secret");
}

#[test]
fn get_reports_failing_source() {
    let daemon = DaemonFixture::start();

    let result = daemon.run(&["get", "api", "exit 3"]);
    assert_ne!(result.status.unwrap(), 0);
    assert!(result.stderr.contains("source command failed"));
}

#[test]
fn purge_secret() {
    let daemon = DaemonFixture::start();

    daemon.set("db", "to-be-purged");

    let result = daemon.run(&["purge", "db"]);
    assert_eq!(result.status.unwrap(), 0);
    assert_eq!(result.stdout.trim(), "OK");

    let result = daemon.run(&["status"]);
    assert!(result.stdout.contains("has_secret: false"));
}

#[test]
fn status_reflects_secret_state() {
    let daemon = DaemonFixture::start();

    let result = daemon.run(&["status"]);
    assert!(result.stdout.contains("has_secret: false"));

    daemon.set("db", "s3cret");

    let result = daemon.run(&["status"]);
    assert!(result.stdout.contains("has_secret: true"));
    assert!(result.stdout.contains("count: 1"));
    assert!(result.stdout.contains("keys: db"));
    assert!(result.stdout.contains("last_accessed_secs_ago: 0"));
}

#[test]
fn secret_survives_multiple_gets() {
    let daemon = DaemonFixture::start();

    daemon.set("db", "persistent");

    for _ in 0..3 {
        let result = daemon.run(&["get", "db"]);
        assert_eq!(result.status.unwrap(), 0);
        assert_eq!(result.stdout.trim(), "persistent");
    }
}

#[test]
fn multiple_keys_are_independent() {
    let daemon = DaemonFixture::start();

    daemon.set("db", "db-secret");
    daemon.set("api", "api-secret");

    assert_eq!(daemon.run(&["get", "db"]).stdout.trim(), "db-secret");
    assert_eq!(daemon.run(&["get", "api"]).stdout.trim(), "api-secret");

    let status = daemon.run(&["status"]);
    assert!(status.stdout.contains("count: 2"));

    // Purging one key leaves the other intact.
    daemon.run(&["purge", "db"]);
    let status = daemon.run(&["status"]);
    assert!(status.stdout.contains("count: 1"));
    assert!(status.stdout.contains("keys: api"));
    assert_eq!(daemon.run(&["get", "api"]).stdout.trim(), "api-secret");
}

#[test]
fn daemon_not_running() {
    let sock = temp_socket_path();
    cleanup_socket(&sock);

    let result = Command::new(cli_bin())
        .arg("--socket")
        .arg(&sock)
        .arg("status")
        .output()
        .expect("failed to run CLI");

    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("cannot connect") || stderr.contains("No such file"));
}
