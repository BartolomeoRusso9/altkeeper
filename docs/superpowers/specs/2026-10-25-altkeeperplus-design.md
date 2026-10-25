# AltKeeperPlus design

Date: 2026-10-25
Fork: https://github.com/WazeJinx/AltKeeperPlus (upstream: BartolomeoRusso9/altkeeper)

## Intent

The owner runs AltKeeper on a Windows 11 PC as `altkeeper.exe serve --altserver --dir C:\altkeeper`
in a cmd window. Two things bother them: the log is half in Italian, and the program lives in a
console window they have to keep open and read. They want AltKeeper to feel like the official
AltServer on Windows: a tray icon, a small window to look at the phone and the apps, and nothing
else on screen. It should stay simple. The existing web page at `127.0.0.1:8787` already has
everything they need (iPhone reachability, apps and renew buttons, install an IPA, Apple ID
sign-in), so the window shows that page rather than a new UI.

## Goals

1. **English logs.** Every user-facing string the program prints or returns (console, web page
   API errors, web job lines) is English.
2. **Timestamps.** Every log line starts with the local time, `[YYYY-MM-DD HH:MM:SS]`.
3. **Tray mode on Windows.** `altkeeper.exe tray --dir C:\altkeeper` (and double-clicking the
   exe with no arguments) runs `serve --altserver` with no console window and a tray icon.
   - Double-click the icon: open the AltKeeper window.
   - Right-click menu: *Open AltKeeper*, *Show logs*, *Start with Windows* (checkbox),
     *Check for updates*, *Update now* (only when one is available), *Quit*.
   - *Start with Windows* is on by default: the first time tray mode runs, it adds the HKCU Run
     entry (`"<exe path>" tray --dir <dir>`) and records that in `state/` so that unticking it is
     respected and never re-enabled. If the exe moves, the entry is rewritten on next start.
   - Icon colour: green when the iPhone was reachable at the last check, grey otherwise.
   - Hover tooltip: number of apps and the nearest expiry, e.g. "3 apps, next expiry in 4 days".
4. **Window.** A native window (WebView2, already on Windows 11) showing `http://127.0.0.1:8787`.
   Closing it hides it; AltServer keeps running. Only *Quit* stops the program.
5. **Logs tab.** The web page gets a *Logs* section/tab: the recent log lines (ring buffer, last
   2,000 lines), updating live (poll every 2 s is enough), with *Copy* and *Clear view*.
   *Show logs* in the tray opens the window on this tab. Lines are also appended to
   `altkeeper.log` in the `--dir` folder, rotated at 5 MB (keep one `.1`).
6. **Notifications.** Windows toast when apps are renewed/installed, when a renewal or install
   fails, and when an update is available.
7. **Update check.** On start and every 24 h, query
   `https://api.github.com/repos/WazeJinx/AltKeeperPlus/releases/latest`. Newer than the running
   version: toast plus *Update now* in the menu. Also query upstream's latest release and, when it
   is newer than the version this fork is based on, show a menu line "Upstream has a new version"
   that opens the upstream releases page (a reminder to sync the fork; never installed directly).
8. **Self-update, on click.** *Update now* downloads `altkeeper.exe` and `altkeeper.exe.sha256`
   from the release, verifies the hash (mismatch: abort, nothing changes), renames the running exe
   to `altkeeper.old.exe`, moves the new one into place, starts it with the same arguments, and
   exits. On start, delete a leftover `altkeeper.old.exe`. If the new process does not come up
   (it writes a "started" marker within 15 s), restore the old exe and restart it.
9. **Release workflow.** A GitHub Action on tag `v*` builds `altkeeper.exe` on `windows-latest`,
   writes its SHA-256, and attaches both to a GitHub Release.

## Non-goals

- No new UI design. The window is the existing page plus the Logs tab.
- No automatic install of updates without a click (a restart drops an in-flight install).
- No installing upstream builds; upstream changes arrive by syncing the fork and tagging.
- No tray on Linux/macOS/Docker. Those builds behave exactly as upstream (with English logs and
  timestamps).
- No real console window in tray mode (closing a console kills the process on Windows).

## Anti-goals

- Anything that could stop AltServer silently: closing the window or the logs must never quit.
- Changing the AltStore protocol behaviour, profile handling or Apple sign-in logic.
- Breaking CLI commands used by `renew.ps1` (`renew --min-days 3` keeps printing to stdout, and
  the English phrases it greps for, `installed on the phone` and `downloading a new profile`,
  stay identical).

## Constraints

- Rust, existing crate. New Windows-only dependencies go under
  `[target.'cfg(windows)'.dependencies]`. Suggested: `tray-icon` + `muda` (tray/menu), `wry` +
  `tao` (window/event loop), `winrt-notification` or `tauri-winrt-notification` (toasts),
  `chrono` (timestamps; or hand-rolled with `std::time` + Windows local offset), `sha2`, and an
  HTTP client already in the tree if possible (isideload pulls `reqwest`; reuse it).
- Tray mode needs no console: build stays a console-subsystem binary (CLI must keep working), and
  tray mode calls `FreeConsole()` at start when launched from Explorer. `--console` keeps the
  console for anyone who wants the old behaviour.
- Logging today: `altserver::log` (one `println!`), ~26 `println!/eprintln!` elsewhere, and
  `web.rs` job lines via `push_line`. Route them through one `log` module that timestamps, writes
  stdout, appends to the file and the in-memory ring buffer.
- Version: Cargo version becomes `0.2.0-plus.1`; the fork-base upstream version is a constant.
- Keep each branch one commit, under ~1,200 lines of diff, stacked in this order:
  1. `chore/english-logs`: translate strings (code comments may stay Italian).
  2. `feat/log-timestamps`: `log` module, timestamps, log file, ring buffer.
  3. `feat/logs-tab`: `/api/logs` endpoint + Logs tab on the page.
  4. `feat/tray-window`: tray, menu, WebView2 window, Start with Windows (HKCU Run key).
  5. `feat/tray-status-notify`: icon colour, tooltip, toasts.
  6. `feat/update-check`: release check (fork + upstream), menu items.
  7. `feat/self-update`: download, verify, swap, restart, rollback.
  8. `ci/release`: release workflow (last; it only matters once a tag is pushed).
  9. `docs/readme-plus`: README section for AltKeeperPlus on Windows.

## Left to the builder

- Exact crate versions and choice between equivalent crates.
- Tray icon artwork (simple generated icons are fine).
- Logs tab layout, as long as it matches the existing page style.
- How the tray reads phone/app state: calling the same functions the web `state` handler uses, or
  polling `127.0.0.1:8787/api/state` locally; whichever is less code.
