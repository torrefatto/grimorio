# contrib: running grimoriod as a scheduled/background service

These are example configurations for keeping `grimoriod` running in the
background, started automatically for your user session. Each is a **per-user**
service, which matches grimorio's model: an in-memory secret store scoped to one
logged-in user.

```
contrib/
├── macos/io.grimorio.grimoriod.plist   launchd LaunchAgent
├── linux/grimoriod.service             systemd user unit
└── windows/Install-GrimorioTask.ps1    Task Scheduler registration
```

## Prerequisites

Build and install the binaries so the service can find `grimoriod`:

```sh
cargo install --path .
# installs `grimorio` and `grimoriod` into ~/.cargo/bin
```

Adjust the binary paths in the example files if you installed elsewhere
(e.g. `/usr/local/bin`).

The daemon listens on a Unix domain socket. By default this is
`~/.grimorio/grimorio.sock`; override it with the `GRIMORIO_SOCKET` environment
variable or `--socket`. The CLI and daemon must agree on the path. The daemon
does **not** create the socket's parent directory, so the examples make sure
`~/.grimorio` exists.

---

## macOS (launchd)

1. Edit `macos/io.grimorio.grimoriod.plist` and replace every `YOUR_USERNAME`
   with your short user name (`id -un`).
2. Create the socket/log directory and install the agent:

   ```sh
   mkdir -p ~/.grimorio
   cp contrib/macos/io.grimorio.grimoriod.plist ~/Library/LaunchAgents/
   launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/io.grimorio.grimoriod.plist
   ```

   On older macOS, use `launchctl load -w ~/Library/LaunchAgents/io.grimorio.grimoriod.plist`.

3. Verify:

   ```sh
   launchctl print gui/$(id -u)/io.grimorio.grimoriod
   grimorio status
   ```

Stop / uninstall:

```sh
launchctl bootout gui/$(id -u)/io.grimorio.grimoriod
rm ~/Library/LaunchAgents/io.grimorio.grimoriod.plist
```

To apply changes after editing the plist, `bootout` then `bootstrap` again.

---

## Linux (systemd user unit)

1. Install the unit:

   ```sh
   mkdir -p ~/.config/systemd/user
   cp contrib/linux/grimoriod.service ~/.config/systemd/user/
   systemctl --user daemon-reload
   systemctl --user enable --now grimoriod.service
   ```

2. Verify:

   ```sh
   systemctl --user status grimoriod.service
   journalctl --user -u grimoriod.service -f
   grimorio status
   ```

Optional — keep the daemon alive after you log out (otherwise the user manager
stops it at logout):

```sh
sudo loginctl enable-linger "$USER"
```

Stop / uninstall:

```sh
systemctl --user disable --now grimoriod.service
rm ~/.config/systemd/user/grimoriod.service
systemctl --user daemon-reload
```

---

## Windows (Task Scheduler)

> **Note:** On Windows the IPC uses a named pipe
> (`\\.\pipe\grimorio-<username>`) instead of a Unix socket; override it with
> `GRIMORIO_SOCKET` or `--socket`. This transport has not yet been exercised on
> a real Windows host, so treat the Windows path as experimental — if you hit
> trouble, running under **WSL** with the Linux instructions above is the
> proven route.

1. Build/install so `grimoriod.exe` exists (edit `$Exe` in the script if needed).
2. Register a task that starts the daemon at logon:

   ```powershell
   powershell -ExecutionPolicy Bypass -File .\contrib\windows\Install-GrimorioTask.ps1
   ```

3. Verify:

   ```powershell
   Get-ScheduledTask -TaskName 'grimoriod'
   Start-ScheduledTask -TaskName 'grimoriod'
   grimorio status
   ```

Uninstall:

```powershell
Unregister-ScheduledTask -TaskName 'grimoriod' -Confirm:$false
```

---

## Notes

- **Idle timeout.** The examples pass `--timeout 300` (purge a secret 5 minutes
  after its last access). Tune per key to taste.
- **Secrets are never persisted.** They live only in the daemon's memory and are
  zeroized on purge, expiry, and shutdown. Restarting the service clears
  everything — you will be prompted again on the next `grimorio get <key>`.
- **Socket permissions.** The daemon creates the socket as `0600` (owner only).
  Keep `~/.grimorio` owner-only as well.
