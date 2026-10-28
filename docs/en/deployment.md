# Deployment

AltKeeper needs a persistent data directory and network access to the iPhone. Choose one deployment
method; do not run multiple AltKeeper instances on the same network unless you know how to isolate
their AltServer advertisements.

## Linux with Docker Compose

The sample Compose file uses host networking for Bonjour discovery and AltStore service
advertisement. This is intended for Linux Docker hosts:

1. Copy [`examples/docker-compose.yml`](../../examples/docker-compose.yml) to a directory such as
   `/srv/altkeeper`.
2. Create `/srv/altkeeper/.env` with `ALTKEEPER_WEB_PIN=<private-pin>` and restrict the file's
   permissions.
3. Put `rp-pairing.plist` in `/srv/altkeeper`. If you use independent renewals, sign in once so
   `account.json` and `state/` are created in the mounted data directory.
4. From that directory, run `docker compose up -d`.

The sample mounts `/srv/altkeeper` at `/data`; pairing, account, state, logs, and profile backups
persist there. The image does not include these files. For optional USB access from the container,
see the commented `usbmuxd` mount in the Compose file. For local Anisette, follow
[Local Anisette server](anisette-server.md).

## Windows

Use the native Windows executable rather than Docker Desktop: AltKeeper relies on Bonjour and
Docker Desktop does not provide the host networking behavior the service needs. Install the
Microsoft Visual C++ build tools if building from source; alternatively download a Windows
executable from [Releases](https://github.com/BartolomeoRusso9/altkeeper/releases). The manually
downloadable package is `AltKeeper-v<version>-Windows-x64.zip`; it contains `AltKeeper.exe`, the
guides, and examples. A standalone binary is also available as
`AltKeeper-v<version>-Windows-x64.exe`; both downloads have matching `.sha256` files. The release
also includes `altkeeper.exe` and `altkeeper.exe.sha256` for the in-app updater; those legacy asset
names are intentionally fixed.

From PowerShell, pairing and starting the services look like this:

```powershell
.\AltKeeper.exe pair-usb
.\AltKeeper.exe serve --altserver
```

Install iTunes or Apple Devices for the local USB mux service. Allow AltKeeper through Windows
Firewall on **Private** networks only. Keep the computer awake for scheduled renewal. Use Task
Scheduler for unattended startup or renewal; see [`examples/renew.ps1`](../../examples/renew.ps1).

The tray mode starts the web page and AltServer without a terminal:

```powershell
.\AltKeeper.exe tray
```

The tray app uses the executable's folder for data by default; `--dir <folder>` changes it. Its
embedded web page listens on loopback. WebView2 is required to show the app window.

## macOS

Download the archive matching your Mac from
[Releases](https://github.com/BartolomeoRusso9/altkeeper/releases):
`AltKeeper-v<version>-macOS-Apple-Silicon-App.zip` for Apple Silicon or
`AltKeeper-v<version>-macOS-Intel-App.zip` for Intel. Move `AltKeeper.app` from the archive to
`/Applications`. The app runs in the menu bar and starts AltServer plus the local web page. Grant
Local Network access when prompted. The app is not notarized; follow the Gatekeeper instructions
in the release notes if macOS blocks the first launch.

The matching command-line archives are labeled `...-macOS-Apple-Silicon-CLI.tar.gz` and
`...-macOS-Intel-CLI.tar.gz`. Linux archives are labeled `...-Linux-x64.tar.gz` or
`...-Linux-ARM64.tar.gz`. Each archive has a matching `.sha256` checksum file.

The app's data directory is `~/Library/Application Support/AltKeeper`. The menu option **Start at
login** controls its LaunchAgent. A command-line build can also run with `cargo build --release`;
see the project scripts for packaging a custom `.app`.

## Data and backups

Keep the data directory persistent and private. Back up `rp-pairing.plist`, `account.json`,
`state/`, and any profile backups securely. Do not copy these files into a public image or commit
them to source control. See [Troubleshooting and security](troubleshooting-and-security.md).
