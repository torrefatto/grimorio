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
    let set_result = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "echo -n '{}' | {} --socket {} set",
            secret,
            cli_bin().display(),
            daemon.socket().display()
        ))
        .output()
        .expect("failed to run set via shell");

    assert!(set_result.status.success(), "set: {}", String::from_utf8_lossy(&set_result.stderr));

    let result = daemon.run(&["get"]);
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
            "echo -n '{}' | {} --socket {} get",
            secret,
            cli_bin().display(),
            daemon.socket().display()
        ))
        .output()
        .expect("failed to run get via shell");

    assert!(result.status.success(), "get: {}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(String::from_utf8_lossy(&result.stdout).trim(), secret);
}

#[test]
fn purge_secret() {
    let daemon = DaemonFixture::start();

    Command::new("sh")
        .arg("-c")
        .arg(format!(
            "echo -n 'to-be-purged' | {} --socket {} set",
            cli_bin().display(),
            daemon.socket().display()
        ))
        .output()
        .expect("set failed");

    let result = daemon.run(&["purge"]);
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

    Command::new("sh")
        .arg("-c")
        .arg(format!(
            "echo -n 's3cret' | {} --socket {} set",
            cli_bin().display(),
            daemon.socket().display()
        ))
        .output()
        .expect("set failed");

    let result = daemon.run(&["status"]);
    assert!(result.stdout.contains("has_secret: true"));
    assert!(result.stdout.contains("last_accessed_secs_ago: 0"));
}

#[test]
fn secret_survives_multiple_gets() {
    let daemon = DaemonFixture::start();

    Command::new("sh")
        .arg("-c")
        .arg(format!(
            "echo -n 'persistent' | {} --socket {} set",
            cli_bin().display(),
            daemon.socket().display()
        ))
        .output()
        .expect("set failed");

    for _ in 0..3 {
        let result = daemon.run(&["get"]);
        assert_eq!(result.status.unwrap(), 0);
        assert_eq!(result.stdout.trim(), "persistent");
    }
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
