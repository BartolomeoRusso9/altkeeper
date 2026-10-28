# Troubleshooting and security

## Common problems

| Symptom | Checks |
| --- | --- |
| Remote pairing is missing | Confirm `rp-pairing.plist` is in the active data directory. Pair again over USB if needed. |
| Pairing fails or reports `early eof` | The phone may no longer trust that pairing. Pair again and keep the phone unlocked during setup. |
| Phone cannot be found or times out | Put the iPhone on Wi-Fi, check both devices are on a reachable network, and verify any fixed `--phone ip:port` address. |
| AltStore says “AltServer could not be found” | Check AltKeeper is running, Local Network permission is enabled for AltStore, and only one AltServer is advertising. Read the server log. |
| AltStore reports Anisette `-45061` or `reprovision required` | Test another Anisette URL in the dashboard. A server change retires the saved identity, so Apple may request verification again. |
| Refresh stalls after `PrepareAppRequest` | The iPhone may still be uploading a large app. Keep AltStore open, use a stable Wi-Fi connection, and watch AltKeeper's transfer log. |
| Apple sign-in returns 429 or 503 | Apple may be rate-limiting requests. Stop retrying in a loop; wait before trying again. |
| An app will not open after profile removal | Restore its saved provisioning profile with `profile-restore`. |
| Local Anisette server fails to start | Check both native library files, the architecture directory, and read permissions. See [Local Anisette server](anisette-server.md). |

For detailed CLI syntax, run `altkeeper --help`. Set `ALTKEEPER_DEBUG=1` for sign-in diagnostics;
share only the specific `[gs #...]` error lines after checking that they contain no private data.
Never enable `RUST_LOG=debug` on logs you plan to share: low-level device logs can contain pairing
keys.

## Protect the data directory

Treat these files and directories as secrets:

- `rp-pairing.plist`: keys used to pair with the iPhone.
- `account.json`: Apple ID and, if saved, its password in clear text.
- `state/`: Anisette identity and AltServer state.
- `notify.conf`: notification topic.
- `profili-salvati/`: backups of removed provisioning profiles.

Restrict filesystem access, encrypt backups, and do not attach these files to issues or chats. For
private vulnerability reporting, follow [SECURITY.md](../../SECURITY.md).

## Network exposure

- **AltServer TCP 49500:** no authentication, like AltServer itself. Do not expose it outside a
  trusted LAN or VPN.
- **Web dashboard TCP 8787:** use `ALTKEEPER_WEB_PIN` on a non-loopback bind. It has no HTTPS; keep
  it on a trusted network.
- **Anisette listener:** has no authentication or TLS, and the dashboard PIN does not protect it.
  Bind it only to trusted interfaces.
- Avoid public port forwarding. Use a private VPN and host firewall rules instead.
