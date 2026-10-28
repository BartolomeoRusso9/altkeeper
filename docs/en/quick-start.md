# Quick start

This guide gets one iPhone paired and reachable. Choose a long-running setup in
[Deployment](deployment.md) after confirming the basic connection.

## Requirements

- iPhone on iOS 17 or later, with Developer Mode enabled.
- AltKeeper running on a machine the iPhone can reach on the same Wi-Fi or VPN.
- A one-time USB connection to create remote pairing.
- AltStore Classic is only needed if you want AltKeeper to act as AltServer.
- A saved Apple ID is only needed for independent renewals or signing an IPA from the dashboard.

## Get AltKeeper

Download a binary from [GitHub Releases](https://github.com/BartolomeoRusso9/altkeeper/releases),
or build it with Rust. On Windows, the `AltKeeper-v<version>-Windows-x64.zip` archive
contains `AltKeeper.exe`. If you choose the standalone `AltKeeper-v<version>-Windows-x64.exe`,
run that versioned filename in place of `AltKeeper.exe` in the commands below, or rename it to
`AltKeeper.exe` first.

To build from source:

```sh
git clone https://github.com/BartolomeoRusso9/altkeeper
cd altkeeper
cargo build --release --locked
```

Release packages use the executable name `AltKeeper` (`AltKeeper.exe` on Windows). A source build
outputs `target/release/altkeeper` (`target\release\altkeeper.exe` on Windows). For platform-specific
installation and persistence, see [Deployment](deployment.md). The commands below assume the
release-package name; if building from source, copy the binary as `AltKeeper` or use its
`target/release/` path.

## Pair the iPhone over USB

Connect, unlock, and trust the iPhone on the computer that runs AltKeeper, then run:

```sh
./AltKeeper pair-usb
./AltKeeper phone
```

On Windows, use `.\AltKeeper.exe`. Pairing writes `rp-pairing.plist` to the data
directory. If AltKeeper will run on another machine, transfer the pairing file securely to that
machine's data directory. Treat it as a secret.

## Start AltServer and the web page

On a trusted home network, start both services:

```sh
ALTKEEPER_WEB_PIN='choose-a-private-pin' ./AltKeeper serve \
  --bind 0.0.0.0:8787 --altserver
```

On Windows, use PowerShell:

```powershell
$env:ALTKEEPER_WEB_PIN = 'choose-a-private-pin'
.\AltKeeper.exe serve --bind 0.0.0.0:8787 --altserver
```

Then:

1. Open `http://<server-address>:8787` to use the dashboard.
2. In AltStore Classic, allow Local Network access and try **Refresh All** with one app first.
3. Keep only one AltServer active on the local network.

Without `--altserver`, `serve` starts only the web service. For local-only access, omit `--bind` or
bind to `127.0.0.1`. A non-loopback web bind requires `ALTKEEPER_WEB_PIN`; the web service has no
HTTPS, so do not publish it directly to the Internet.

## Phone discovery

AltKeeper normally discovers the iPhone automatically. To set a fixed address, pass
`--phone ip:port` or set `ALTKEEPER_PHONE=ip:port`; the pairing service usually uses port `49152`.
The iPhone must be on Wi-Fi for its remote pairing service to be reachable.

CLI messages default to English. Set `ALTKEEPER_LANG=it` for Italian; this does not change the
dashboard language.

## Next steps

- [Deployment](deployment.md) for Docker, Windows, and macOS startup options.
- [Accounts and renewals](accounts-and-renewals.md) to sign in and configure renewals.
- [Web and JIT](web-and-jit.md) for dashboard actions and JIT prerequisites.
- [Troubleshooting and security](troubleshooting-and-security.md) before exposing any service.
