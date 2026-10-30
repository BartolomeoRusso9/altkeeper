# Changelog

## Unreleased

### Added
- Automatic renewal can optionally register missing App IDs before downloading their profiles.
  The setting is off by default and the dashboard warns that registrations use the free Apple ID's
  10-per-7-days App ID quota.
- Built-in local Anisette server on `127.0.0.1:8788`, selectable from the tray and the web page.
  On Windows and macOS the native Anisette libraries are downloaded automatically when missing;
  elsewhere they are looked for in `anisette-libraries/`. The web page can test any Anisette URL
  before saving it.
- Telegram notifications for failed automatic renewals (bot token and chat ID are saved privately
  in `state/telegram-bot.json`, with a test button on the dashboard).
- Autostart switch on the dashboard (Windows and macOS). Autostart now launches the tray with
  `--minimized`, so no window opens at login.
- A second launch of the tray app brings the existing window forward (or opens the dashboard)
  instead of failing because the port is already in use.
- Remove several profiles or profile backups at once; the dashboard shows the host's LAN address.
- Automatic renewal can ask for the Apple 2FA code through the web page.

### Fixed
- Sign-in no longer fails when Apple rejects the SMS 2FA request with HTTP 403 and an empty
  body: the vendored isideload now logs a warning (without the response payload) and falls
  back to trusted-device 2FA (push + 6-digit code). Other errors are still propagated.
- Windows: the POSIX shim used by the Anisette libraries now converts paths for `chmod` and `mkdir`,
  honours `O_TRUNC`, creates files with an explicit mode, and no longer mangles `stat` timestamps.
- `anisette_server.rs` uses LF line endings and `\x` escapes for the ZIP signatures instead of raw
  control bytes; `*.rs` is pinned to LF in `.gitattributes`.

### Changed
- Dashboard redesigned (dark mode, collapsible quick guide); the tray window is larger and
  centred on the primary screen.
- When sideloading AltStore itself, `ALTBundleIdentifier` is written to Info.plist, as the official
  AltServer does, so AltStore can refresh itself.
- Automatic renewal opens the Apple session in the data directory instead of the current directory.

## 0.4.0

### Added
- **Anisette server selection.** `https://ani3.stikstore.app` is now the default, with the
  isideload public endpoint and custom HTTP(S) URLs selectable from the web page or tray.
  `ALTKEEPER_ANISETTE_URL` takes precedence; changing the effective server retires the saved
  anisette identity so it can be provisioned again. The page can test the selected server.
- **USB pairing from the dashboard and tray**, plus automatic pairing of newly connected iPhones.
  The monitor uses usbmuxd's device events with a polling fallback, never overwrites a valid
  pairing automatically, and can be disabled with `ALTKEEPER_NO_AUTO_PAIR=1` or the page switch.
- **Saved iPhone addresses.** Store up to two home/VPN IP:port addresses in the web interface;
  they are tried before Bonjour and LAN discovery.
- **Renewal wait and cancel.** If the phone does not acknowledge a downloaded profile, AltKeeper
  checks independently whether it arrived, then waits for the phone to respond (up to 12 hours).
  The dashboard can cancel the wait.
- The log viewer pauses polling in hidden tabs, times out requests, reports copy results, and
  supports clipboard fallback over plain HTTP.

### Changed
- USB pairing now uses the same local/forwarded usbmuxd selection as USB connections and writes
  its pairing file through a private temporary file before replacing the destination.
- The web check timeout allows for both saved addresses.

## 0.3.1

### Added
- **macOS menu-bar app (`AltKeeper.app`).** Double-clicking the app now starts `altkeeper tray`,
  the same program as the Windows tray: an icon in the menu bar like AltServer (no Dock icon,
  `LSUIElement`), grey while the iPhone is unreachable, with *Open AltKeeper* (the web page in a
  WKWebView window), *Show logs*, *Start at login* (a LaunchAgent in
  `~/Library/LaunchAgents/io.github.altkeeper.plist`, on by default the first time, refused while
  the app runs from App Translocation), *Check for updates* (opens the release page: the signed app
  does not replace itself) and *Quit*. Notifications go to Notification Center. Data lives in
  `~/Library/Application Support/AltKeeper`. On macOS the event loop runs on the process's main
  thread, as AppKit requires; the tokio runtime stays on `altkeeper-main`.
- `scripts/macos-app.sh` builds `AltKeeper.app` (icon, `Info.plist` with the Local Network usage
  description and Bonjour services, ad-hoc signature); the release workflow uses it.

### Changed
- `release.yml`: the macOS `.app` is no longer a shell launcher that opened the browser and could
  only be stopped with Force Quit. The macOS jobs now also run the tests, and the duplicated header
  comment was removed.
- The tray icon is created once the event loop has started, on Windows too (required on macOS).
- READMEs: new "macOS menu-bar app" and "Releases" sections.

## 0.3.0

### Fixed
- **Tray panicked on Windows ("Initializing the event loop outside of the main thread", #19).**
  The tao event loop is now created with `with_any_thread(true)`, because `main()` runs everything
  on the `altkeeper-main` thread. Not compiled on Windows here: please test `altkeeper.exe tray`.
- **Log file is private and auto-renew errors reach it.** `altkeeper.log` (and its rotation) is
  created with owner-only permissions, and the auto-renew errors that went to stderr (invisible in
  the tray) now go through the log.
- **USB on Windows without configuration.** When `USBMUXD_SOCKET_ADDRESS` is not set, Windows now
  uses the usbmuxd of iTunes / Apple Devices on `127.0.0.1:27015` instead of failing to find the
  phone. Ported from the USB-fix branch.
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
- **Windows tray icon (`altkeeper tray`, also what a double-click on `altkeeper.exe` starts).**
  AltServer and the web page run without a terminal, with an icon next to the clock (AltKeeper
  logo, grey while the iPhone is unreachable), a menu (*Open AltKeeper*, *Show logs*, *Start with
  Windows*, *Check for updates*, *Quit*), a window with the web page (WebView2) and toast
  notifications for renewed profiles, installed apps and failures. Ported from AltKeeperPlus.
- **Updates from the tray and `release.yml`.** The icon checks the fork's GitHub releases once a
  day; *Update now* verifies the SHA-256, restarts and rolls back if the new version does not
  start. `release.yml` builds `altkeeper.exe` for Windows on a `v*` tag. Ported from AltKeeperPlus.
- **Persistent logs.** Server lines (AltServer, web page, DNS-SD) carry a timestamp and are
  also written to `altkeeper.log` (rotated at 5 MB) and shown in a new **Logs** section of the
  web page (`/api/logs`). Ported from AltKeeperPlus.
- **AltKeeper logo as the icon of `altkeeper.exe`** (embedded by the new `build.rs` on Windows builds, from `assets/altkeeper.ico`), besides the tray and window icons.
- **App / App ID counters** next to "Your apps" in the web page (3 active apps and 10 App IDs for
  a free account; an estimate from the profiles on the phone). Ported from the USB-fix branch.
- **USB when the iPhone is plugged into the machine running AltKeeper.** Every connection (check, renew, sign and install, AltServer) first looks for an iPhone on USB through the local `usbmuxd` (or the one in `USBMUXD_SOCKET_ADDRESS`) and talks to it through the CoreDeviceProxy tunnel; if there is none, or it fails, it falls back to Wi-Fi as before. Much faster for installing IPAs. Needs the phone to already trust the computer (the usual "Trust" prompt), not the remote pairing. `--phone <ip:port>` forces Wi-Fi; `ALTKEEPER_NO_USB=1` turns USB off. In Docker the usbmuxd socket must be mounted into the container.
- `ALTKEEPER_REVOKE_CERTS=1`: when installing an IPA from the web page fails with Apple error 7460 (the Apple ID already has a development certificate, e.g. from AltStore/SideStore), AltKeeper can revoke it and create its own. Off by default; without it you now get a clear explanation instead of a generic error.
- **Windows support (native, no Docker).** `cargo build --release` on Windows 10/11 produces
  `altkeeper.exe`. Everything that was Unix-only is now portable: private files get an `icacls`
  ACL instead of `chmod 600`/`700`, random numbers come from the OS through the `rustls` provider
  (no more `/dev/urandom`), the local time in the web page's log comes from PowerShell, and
  `altserver` also stops on console close / shutdown, not only Ctrl-C. New
  `examples/renew.ps1` (the Windows counterpart of `renew.sh`, with the same ntfy notifications)
  and `.gitattributes` so the shell scripts keep LF endings in a Windows checkout. See the
  README, section "Windows". **Confirmed on a real Windows 10 machine:**
  `cargo test --locked` (63 passed), `cargo build --release --locked`, English `--help`
  and interactive PowerShell Apple ID / 2FA login. Windows 11 and complete AltServer /
  unattended-renewal workflows still need end-to-end testing.
- **Works from outside your home network.** `--dns-domain` publishes the AltServer service as
  ordinary DNS records (unicast DNS-SD), so AltStore discovers it through a VPN. iOS never sends
  Bonjour multicast over a VPN interface, but it does send plain DNS queries, and AltStore browses
  the device's search domains. Includes a small built-in DNS responder (UDP, answers only for its
  own names); put it behind the resolver you already run. See the README for the full setup.
- **English/Italian CLI** with `ALTKEEPER_LANG=en` or `ALTKEEPER_LANG=it`, default Italian;
  regional variants and `ALTREFRESH_LANG` alias supported. Localize CLI prompts, login/2FA,
  errors, server logs and `--help` without modifying protocol responses, JSON or the web UI.
- Added Windows/Linux locale smoke checks, Rust tests and manual Windows 2FA test guidance.

### Fixed
- **Windows PowerShell Apple ID 2FA input.** Read directly from the attached Windows console
  (`CONIN$`) rather than native-process standard input, which can be unavailable in PowerShell.
  Handle EOF instead of submitting an empty verification code; validate six-digit codes,
  leading zeroes, device/SMS choices, and use the same mechanism for team selection and
  the password-save prompt.

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
