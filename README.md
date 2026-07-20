# grimorio

A small daemon that keeps secrets encrypted in memory, plus a CLI to store and
retrieve them. Secrets are held only in the daemon's process memory (AES-256-GCM
encrypted, zeroized on purge, expiry, and shutdown) and are never written to
disk. The two talk over a per-user local IPC channel.

- `grimoriod` — the daemon (the store)
- `grimorio` — the CLI (talks to the daemon)

## Build

```sh
cargo build --release
# binaries: target/release/grimorio and target/release/grimoriod
```

Or install both into `~/.cargo/bin`:

```sh
cargo install --path .
```

## Run the daemon

```sh
grimoriod
```

It listens on a per-user endpoint and purges any secret 5 minutes (300s) after
its last access by default. To keep it running in the background across logins,
see [`contrib/`](contrib/) for launchd / systemd / Task Scheduler examples.

## Usage

```sh
# Store a secret under a key (read from stdin)
printf 'my-secret' | grimorio set db

# Retrieve it
grimorio get db
# -> my-secret

# Show what the daemon is holding
grimorio status

# Clear one key, or every secret when the key is omitted
grimorio purge db
grimorio purge
```

### Fallback source for `get`

`get` takes an optional second argument: a shell command evaluated only when no
secret is stored under the key. Its stdout becomes the secret, which is cached
in the daemon and printed:

```sh
grimorio get gh-token 'gh auth token'
grimorio get db "op read 'op://Private/db/password'"
```

- If a secret is already stored under the key, the source is ignored (never run).
- With no source and no stored secret, the CLI prompts on the terminal instead.
- The command runs through your login shell (`$SHELL -i`) so aliases and rc-file
  functions are available. It inherits the terminal for stdin/stderr, so an
  interactive source (e.g. a password manager prompting for a master password or
  a hardware-key touch) can reach you. A non-zero exit fails the `get`.

## Configuration

| Flag | Env | Default | Meaning |
| --- | --- | --- | --- |
| `--socket <PATH>` | `GRIMORIO_SOCKET` | see below | IPC endpoint; CLI and daemon must agree |
| `--timeout <SECS>` | — | `300` | idle seconds before a secret is purged |

Default endpoint:

- Unix: `~/.grimorio/grimorio.sock` (a Unix domain socket, created `0600`). The
  daemon does not create the parent directory — `mkdir -p ~/.grimorio` first.
- Windows: `\\.\pipe\grimorio-<username>` (a named pipe).

## Security notes

- Secrets live only in daemon memory and are zeroized on purge, expiry, and
  shutdown. Restarting the daemon clears everything.
- Output goes to stdout; logs and prompts go to stderr, so `grimorio get` is safe
  to use in command substitution.

## Tests

```sh
cargo test
```
