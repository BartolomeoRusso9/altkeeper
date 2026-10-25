# AltKeeperPlus Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** English, timestamped logs everywhere, plus a Windows tray app (WebView2 window over the existing web page, Logs tab, status icon, toasts, update check and self-update) for AltKeeper.

**Architecture:** One new `log` module becomes the single sink for user-facing output (stdout + `altkeeper.log` + in-memory ring buffer exposed at `/api/logs`). A Windows-only `tray` module runs the `tao` event loop on the main thread (via `block_in_place`) while `serve --altserver` runs on the tokio runtime; the window is a `wry` WebView of `http://127.0.0.1:8787`. A Windows-only `update` module checks GitHub releases and swaps the exe.

**Tech Stack:** Rust 2021, tokio, axum 0.8 (existing); new Windows-only: `tray-icon`, `muda`, `tao`, `wry`, `tauri-winrt-notification`, `winreg`, `windows-sys` (FreeConsole). Cross-platform reuse from the lockfile: `chrono` (local time), `reqwest` (blocking + rustls), `sha2`.

**Spec:** `docs/superpowers/specs/2026-10-25-altkeeperplus-design.md`

## Global Constraints

- One branch = one commit, < ~1,200 diff lines, stacked in spec order; no `Co-Authored-By` trailer.
- Windows-only crates under `[target.'cfg(windows)'.dependencies]`; Linux/macOS/Docker builds keep upstream behaviour (apart from English + timestamps).
- Timestamp format: `[YYYY-MM-DD HH:MM:SS]` local time, then a space, then the line.
- `renew` keeps printing to stdout; phrases `installed on the phone` and `downloading a new profile` unchanged.
- Log file `altkeeper.log` in the working dir (`--dir`), rotate at 5 MB to `altkeeper.log.1`; ring buffer 2,000 lines.
- Update URLs: `https://api.github.com/repos/WazeJinx/AltKeeperPlus/releases/latest`, upstream `https://api.github.com/repos/BartolomeoRusso9/altkeeper/releases/latest`; release assets `altkeeper.exe`, `altkeeper.exe.sha256`.
- Version `0.2.0-plus.1`; `const UPSTREAM_BASE: &str = "0.2.0";`.
- Start with Windows: HKCU `Software\Microsoft\Windows\CurrentVersion\Run`, value `AltKeeperPlus` = `"<exe>" tray --dir "<dir>"`; default on once, tracked by `state/autostart-initialised`.
- Before each commit: `cargo fmt --check`, `cargo clippy --all-targets`, `cargo test`, `cargo build --release`.

## Review Focus

1. Closing the window or the Logs tab never stops AltServer: covered by Task 4 manual check (close window, AltStore *Refresh* still answered).
2. A second instance (ports 49500/8787 busy) in tray mode shows a toast/message box with the bind error instead of dying silently: Task 4 test `tray_reports_bind_error`.
3. Corrupt or partial update download (hash mismatch, 404, no network) leaves the running exe untouched: Task 7 tests `update_rejects_bad_hash`, `update_offline_is_noop`.
4. Version compare with prerelease suffixes (`0.2.0-plus.2` > `0.2.0-plus.1`, `0.3.0` > `0.2.0-plus.9`): Task 6 test `version_ordering`.
5. Log file not writable (read-only dir) must not crash logging: Task 2 test `log_survives_unwritable_file`.

---

### Task 1: English user-facing strings (`chore/english-logs`)

**Files:** Modify `src/*.rs` string literals in `println!`/`eprintln!`/`format!`/`Err(...)`/`log(...)`/`push_line(...)`, `USAGE`, `src/web/index.html` (any Italian text). Leave comments.

- [ ] Step 1: list candidates: `rg -n '"[^"]*[àèéìòù]|\b(non |sul |della |profilo|telefono|errore|attendo|ricevut|connesso|chiuso)' src` and review every user-facing hit.
- [ ] Step 2: translate, using the table agreed in chat (e.g. `connesso`→`connected`, `nessuna richiesta entro il tempo limite`→`no request within timeout`, `non riesco ad ascoltare su {bind}: {e}`→`cannot listen on {bind}: {e}`). Keep placeholders and argument order.
- [ ] Step 3: update any test asserting an Italian string to the English one; `renew.ps1`-matched phrases unchanged.
- [ ] Step 4: `cargo test` PASS; re-run Step 1 grep, expect only comments.
- [ ] Step 5: commit `chore: translate user-facing messages to English`.

### Task 2: `log` module with timestamps, file and ring buffer (`feat/log-timestamps`)

**Files:** Create `src/log.rs`; modify `src/main.rs` (mod + init), `src/altserver.rs::log`, all `println!`/`eprintln!` that are log-like (not `version`, not `USAGE`), `src/web.rs::App::push_line` (also calls `log::line`).

**Interfaces — Produces:**
- `pub fn init(dir: &Path)` — open/rotate `altkeeper.log`; call after `--dir` is applied.
- `pub fn line(msg: &str)` — timestamp, print to stdout, append to file (errors ignored), push to ring.
- `pub fn err(msg: &str)` — same, to stderr.
- `pub fn since(seq: u64) -> (u64, Vec<String>)` — lines after `seq`, and the latest seq.
- `pub fn stamp(now: chrono::DateTime<chrono::Local>, msg: &str) -> String`.

- [ ] Step 1: tests in `src/log.rs`: `stamp_format` (`stamp(2026-10-25 14:03:22, "x") == "[2026-10-25 14:03:22] x"`), `ring_keeps_last_2000` (push 2,005, `since(0).1.len()==2000`, first is line 6), `since_returns_only_new`, `rotates_at_5mb` (tempdir, write > 5 MB, expect `.1` exists), `log_survives_unwritable_file` (init on a path that is a directory; `line` does not panic).
- [ ] Step 2: run `cargo test log::` — FAIL (module missing).
- [ ] Step 3: implement with `static RING: Mutex<VecDeque<(u64,String)>>`, `static FILE: Mutex<Option<File>>`.
- [ ] Step 4: switch call sites; `altserver::log` becomes `log::line(&format!("[altserver] {msg}"))`.
- [ ] Step 5: `cargo test` PASS; run `altkeeper version` (no timestamp) and `altkeeper serve` (timestamped lines).
- [ ] Step 6: commit `feat: timestamped logs with log file and in-memory buffer`.

### Task 3: Logs tab (`feat/logs-tab`)

**Files:** Modify `src/web.rs` (route `GET /api/logs?since=N` → `{"seq":u64,"lines":[..]}`), `src/web/index.html` (Logs section: `<pre id="logs">`, *Copy*, *Clear view*, poll every 2 s, open when URL hash is `#logs`).

**Interfaces — Consumes:** `log::since`.

- [ ] Step 1: test `api_logs_returns_new_lines` (axum oneshot: log two lines, GET `/api/logs?since=0`, expect both; GET with returned seq, expect empty).
- [ ] Step 2: FAIL; Step 3: implement; Step 4: PASS; manual: page shows lines live, `#logs` jumps there.
- [ ] Step 5: commit `feat: logs tab on the web page`.

### Task 4: Tray + window + autostart (`feat/tray-window`)

**Files:** Create `src/tray.rs` (`#[cfg(windows)]`); modify `src/main.rs` (commands `tray`, and no-args on Windows → `tray`), `Cargo.toml` (windows deps), add `assets/tray-green.ico`, `assets/tray-grey.ico` (generated, 32×32).

**Interfaces — Produces:**
- `pub fn run(dir: PathBuf, open_logs_first: bool) -> !` — builds tao loop, tray, menu; never returns.
- `pub fn autostart_enabled() -> bool`, `pub fn set_autostart(on: bool, exe: &Path, dir: &Path) -> io::Result<()>`.
- `pub fn ensure_autostart_default(state_dir: &Path, exe: &Path, dir: &Path)` — first run: enable + write marker; later: rewrite path if exe moved and entry exists.

- [ ] Step 1: tests (Windows-only): `autostart_roundtrip` (set on/off under a test value name), `autostart_default_only_once` (marker present + entry removed → stays removed), `tray_reports_bind_error` (helper `start_server(bind)` returns `Err` containing `cannot listen on` when port is taken).
- [ ] Step 2: FAIL; Step 3: implement. `tray` command: unless `--console`, call `FreeConsole()`; `log::init`; start altserver + `web::serve` on tokio (`tokio::spawn`); on serve error → toast + `MessageBoxW`; then `tokio::task::block_in_place(|| tray::run(..))`. Menu: Open AltKeeper, Show logs, Start with Windows (check), Check for updates (disabled until Task 6), Quit (`altserver::stop()`, `std::process::exit(0)`). Double-click → open window. Window close → hide.
- [ ] Step 4: PASS; manual on Windows: double-click exe → no console, tray icon, window opens, closing hides, AltStore *Refresh* works with window closed, *Quit* frees port 49500.
- [ ] Step 5: commit `feat: Windows tray icon with app window and start with Windows`.

### Task 5: Status icon, tooltip, toasts (`feat/tray-status-notify`)

**Files:** Modify `src/tray.rs`; small hook in `src/web.rs` (job outcome callback) and `src/altserver.rs` (install success/failure → `log` event).

**Interfaces — Produces:** `pub fn summary(apps: &[(String, i64)]) -> String` (e.g. `"3 apps, next expiry in 4 days"`, `"no apps"`); `pub enum Event { Renewed(Vec<String>), Failed(String), UpdateAvailable(String) }` sent via `tao::EventLoopProxy`.

- [ ] Step 1: tests `summary_text` (0, 1 → `"1 app, next expiry in 1 day"`, 3 apps), `failed_toast_rate_limited` (two failures within 24 h → one toast).
- [ ] Step 2: FAIL; Step 3: implement: every 10 min (and after each job) read the same data `/api/state` uses; set icon green/grey + tooltip; toasts for `Renewed`, `Failed`.
- [ ] Step 4: PASS; manual: phone off Wi‑Fi → grey within 10 min.
- [ ] Step 5: commit `feat: tray status icon, tooltip and notifications`.

### Task 6: Update check (`feat/update-check`)

**Files:** Create `src/update.rs` (cross-platform logic, Windows wiring in `tray.rs`).

**Interfaces — Produces:** `pub fn newer(current: &str, latest_tag: &str) -> bool`; `pub fn check() -> Result<Option<Release>, String>` where `Release { tag: String, exe_url: String, sha_url: String, page: String }`; `pub fn upstream_newer() -> Result<Option<String>, String>` (compares with `UPSTREAM_BASE`).

- [ ] Step 1: tests `version_ordering` (`newer("0.2.0-plus.1","v0.2.0-plus.2")`, `newer("0.2.0-plus.9","v0.3.0")`, `!newer("0.2.0-plus.1","v0.2.0-plus.1")`, `!newer(.., "garbage")`), `parse_release_json` (fixture JSON → `Release` with both asset URLs; missing asset → `None`).
- [ ] Step 2: FAIL; Step 3: implement (blocking reqwest, `User-Agent: AltKeeperPlus`, 20 s timeout); tray checks at start + every 24 h; enables *Update now*, toast; upstream line opens releases page.
- [ ] Step 4: PASS. Step 5: commit `feat: check GitHub releases for updates`.

### Task 7: Self-update (`feat/self-update`)

**Files:** Modify `src/update.rs`, `src/tray.rs`, `src/main.rs` (startup: delete `altkeeper.old.exe`, write `state/started` marker).

**Interfaces — Produces:** `pub fn apply(rel: &Release, exe: &Path, args: &[String]) -> Result<(), String>`; `pub fn verify(bytes: &[u8], sha_file: &str) -> bool`.

- [ ] Step 1: tests `update_rejects_bad_hash` (exe file unchanged), `update_offline_is_noop` (unreachable URL → `Err`, exe unchanged), `swap_and_rollback` (tempdir fake exe: swap, simulate no marker → old restored).
- [ ] Step 2: FAIL; Step 3: implement: download to `altkeeper.new.exe`, verify, rename running → `.old.exe`, rename new → exe, spawn with same args, wait up to 15 s for `state/started` newer than spawn time, else rename back and respawn old; on success exit.
- [ ] Step 4: PASS; manual with a test tag on the fork.
- [ ] Step 5: commit `feat: self-update from GitHub releases`.

### Task 8: Release workflow (`ci/release`)

**Files:** Create `.github/workflows/release.yml`: on `push: tags: ['v*']`, `windows-latest`, `cargo build --release`, `certutil`/`Get-FileHash` → `altkeeper.exe.sha256` (hex only), `softprops/action-gh-release` uploads both.

- [ ] Step 1: commit; Step 2 (after merge): push tag `v0.2.0-plus.1`, confirm release has both assets.

### Task 9: README (`docs/readme-plus`)

**Files:** Modify `README.md`: top section "AltKeeperPlus on Windows" (tray, logs tab, updates, autostart, one-instance warning).

- [ ] Step 1: commit `docs: AltKeeperPlus Windows section`.
