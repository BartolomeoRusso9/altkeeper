# Changelog

## Unreleased

### Fixed
- **Install hung forever over USB on iOS 27.** On USB, installing an IPA now goes through AFC + installation_proxy via usbmuxd (same path as `pymobiledevice3 apps install`) instead of the CoreDeviceProxy software tunnel, and the profile client prefers the lockdown service too. App install and profile install now have timeouts, so a stuck phone produces an error instead of a silent hang. The status line says "over USB" or "over Wi-Fi" according to the real transport. Not compiled in this environment: please run `cargo build` and test.

### Changed
- **Sign-in works again (Apple `-80045` since 2026-10-12).** The vendored `isideload` was updated to
  **0.4.6** (upstream `8ca39b62`), which takes the anisette `client_info` from the anisette server
  and resyncs / re-provisions when Apple asks (nab138/isideload#26 and follow-ups). Only two call
  sites needed adapting: `MaxCertsBehavior` is now generic (we use the crate's
  `MaxCertsCallbackBox`), and `sign_app` gained `pairing_file`/`device_udid` (passed as `None`
  here). The `AnisetteData` accessors used by the AltStore anisette bridge were restored in the
  vendored copy. The local `ALTKEEPER_DEBUG` logging and the HTTP/1.1 + `Connection: close`
  transport tweaks were ported to the new copy. Tested on a long-running Docker deployment:
  unattended renewals (profiles + App IDs) work again.

### Added
- **USB when the iPhone is plugged into the machine running AltKeeper.** Every connection (check, renew, sign and install, AltServer) first looks for an iPhone on USB through the local `usbmuxd` (or the one in `USBMUXD_SOCKET_ADDRESS`) and talks to it through the CoreDeviceProxy tunnel; if there is none, or it fails, it falls back to Wi-Fi as before. Much faster for installing IPAs. Needs the phone to already trust the computer (the usual "Trust" prompt), not the remote pairing. `--phone <ip:port>` forces Wi-Fi; `ALTKEEPER_NO_USB=1` turns USB off. In Docker the usbmuxd socket must be mounted into the container.
- `ALTKEEPER_REVOKE_CERTS=1`: when installing an IPA from the web page fails with Apple error 7460 (the Apple ID already has a development certificate, e.g. from AltStore/SideStore), AltKeeper can revoke it and create its own. Off by default; without it you now get a clear explanation instead of a generic error.
- **Windows support (native, no Docker).** `cargo build --release` on Windows 10/11 produces
  `altkeeper.exe`. Everything that was Unix-only is now portable: private files get an `icacls`
  ACL instead of `chmod 600`/`700`, random numbers come from the OS through the `rustls` provider
  (no more `/dev/urandom`), the local time in the web page's log comes from PowerShell, and
  `altserver` also stops on console close / shutdown, not only Ctrl-C. New
  `examples/renew.ps1` (the Windows counterpart of `renew.sh`, with the same ntfy notifications)
  and `.gitattributes` so the shell scripts keep LF endings in a Windows checkout. See the
  README, section "Windows". **Not yet built or run on a real Windows machine**: please report
  anything that breaks.
- **Works from outside your home network.** `--dns-domain` publishes the AltServer service as
  ordinary DNS records (unicast DNS-SD), so AltStore discovers it through a VPN. iOS never sends
  Bonjour multicast over a VPN interface, but it does send plain DNS queries, and AltStore browses
  the device's search domains. Includes a small built-in DNS responder (UDP, answers only for its
  own names); put it behind the resolver you already run. See the README for the full setup.

### Documentation
- "If something goes wrong" now covers the anisette error `not provisioned for this server
  (-45061)` (switch server with `ALTKEEPER_ANISETTE_URL`) and a refresh that looks stuck while
  AltStore sends the app to the server.

## 0.2.0

### Added
- **AltServer for AltStore Classic** (`altkeeper altserver`, or `serve --altserver`): AltStore's
  *Refresh All* and installing, updating or removing apps work with this machine answering instead
  of AltServer on a computer. It announces itself over Bonjour and speaks AltStore's own protocol
  (port 49500 by default, `--altserver-port` to change it).
- **Web page** (`serve`) in Italian for people who are not comfortable with a terminal: state of the
  apps, renew, sign in with the Apple ID (including the verification code). Protected by a PIN
  (`ALTKEEPER_WEB_PIN`), which is required when it listens outside the machine itself.
- **Docker image** (`Dockerfile`, `examples/docker-compose.yml`) and a GitHub Actions workflow that
  tests the code and publishes the image to ghcr.io on tags.
- The iPhone is found by itself: last known address, Bonjour, then a scan of the local network.
- `ALTKEEPER_ANISETTE_URL` to use your own anisette server.
- `altkeeper --version`.
- Notifications (ntfy) and cron examples in `examples/`.

### Changed
- **Renamed from altrefresh to AltKeeper.** The binary, the image and the variables are now
  `altkeeper` / `ALTKEEPER_*`. The old `ALTREFRESH_*` variable names still work.
- `renew` renews per app and signs in to Apple only when something is actually due.

### Fixed
- Sign-in failing with HTTP 429: Apple's servers limit the requests on one connection, so each
  request now uses its own connection. The client also presents itself as a Mac, like AltServer.
- AltStore reporting "AltServer could not be found" while another operation was running: the phone
  is locked only while it is in use.
- Bonjour answers going missing when two discovery daemons ran in the same process.

### Known limitations
- Only one AltServer should run on the network at a time.
- Tested on an iPhone 15 Pro (iOS 27.0) with AltStore Classic; it relies on private Apple APIs and
  on AltStore's protocol, which can change without notice.
- The AltServer port has no login, exactly like the real AltServer: keep it inside your home network.

## 0.1.0

- First version: renews an iPhone's provisioning profiles over Wi-Fi with a free Apple ID, using
  remote pairing, from a machine that stays on at home.
